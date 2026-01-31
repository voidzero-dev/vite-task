use std::{
    io::{Read, Write},
    sync::{Arc, Mutex},
    thread,
};

pub use portable_pty::CommandBuilder;
use portable_pty::{ChildKiller, PtyPair};

use crate::geo::ScreenSize;

/// A headless terminal
pub struct Terminal {
    pty_pair: PtyPair,
    parser: vt100::Parser<Vt100Callbacks>,
    child_killer: Box<dyn ChildKiller + Send + Sync>,
    reader: Box<dyn Read + Send>,
    buffer: Vec<u8>,
}

struct Vt100Callbacks {
    writer: Arc<Mutex<Option<Box<dyn Write + Send>>>>,
}

impl vt100::Callbacks for Vt100Callbacks {
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        // CSI 6 n = Device Status Report (cursor position query)
        // Response: ESC [ Pl ; Pc R
        if let Some(&[6]) = params.first()
            && i1.is_none()
            && i2.is_none()
            && c == 'n'
        {
            let (row, col) = screen.cursor_position();
            let response = format!("\x1b[{};{}R", row + 1, col + 1);
            if let Some(writer) = self.writer.lock().unwrap().as_mut() {
                let _ = writer.write_all(response.as_bytes());
            }
        }
    }
}

impl Terminal {
    pub fn spawn(size: ScreenSize, cmd: CommandBuilder) -> anyhow::Result<Self> {
        let pty_pair = portable_pty::native_pty_system().openpty(portable_pty::PtySize {
            rows: size.rows,
            cols: size.cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        // Create reader BEFORE spawning child to ensure it's ready for data
        let reader = pty_pair.master.try_clone_reader()?;
        let mut child = pty_pair.slave.spawn_command(cmd)?;
        let child_killer = child.clone_killer();
        let writer: Arc<Mutex<Option<Box<dyn Write + Send>>>> =
            Arc::new(Mutex::new(Some(pty_pair.master.take_writer()?)));

        // Background thread: wait for child to exit, then close writer to trigger EOF
        let writer_clone = Arc::clone(&writer);
        thread::spawn(move || {
            let _ = child.wait();
            // Close writer to signal EOF to the reader
            *writer_clone.lock().unwrap() = None;
        });

        Ok(Self {
            pty_pair,
            parser: vt100::Parser::new_with_callbacks(
                size.rows,
                size.cols,
                0,
                Vt100Callbacks { writer },
            ),
            child_killer,
            reader,
            buffer: Vec::new(),
        })
    }

    /// Read data from buffer and reader as a unified stream.
    /// Returns (bytes_read, is_eof) where bytes_read is the number of new bytes added to buffer.
    fn read(&mut self) -> anyhow::Result<(usize, bool)> {
        let mut buffer = [0u8; 4096];
        let n = self.reader.read(&mut buffer)?;

        if n == 0 {
            return Ok((0, true));
        }

        // Process data through parser immediately (important for Windows)
        self.parser.process(&buffer[..n]);

        // Append to persistent buffer
        self.buffer.extend_from_slice(&buffer[..n]);

        Ok((n, false))
    }

    /// Read until the expected string is found in the terminal output.
    pub fn read_until(&mut self, expected: &str) -> anyhow::Result<()> {
        let expected_bytes = expected.as_bytes();

        loop {
            // Check if expected string is in buffer
            if let Some(pos) = self
                .buffer
                .windows(expected_bytes.len())
                .position(|window| window == expected_bytes)
            {
                let split_pos = pos + expected_bytes.len();
                // Keep only the unprocessed remainder in buffer
                self.buffer = self.buffer[split_pos..].to_vec();
                return Ok(());
            }

            // Read more data
            let (_, is_eof) = self.read()?;
            if is_eof {
                return Err(anyhow::anyhow!("Expected string not found: {}", expected));
            }
        }
    }

    pub fn kill(&mut self) -> anyhow::Result<()> {
        self.child_killer.kill()?;
        Ok(())
    }

    pub fn read_to_end(&mut self) -> anyhow::Result<String> {
        // Read all remaining data until EOF
        loop {
            let (_, is_eof) = self.read()?;
            if is_eof {
                break;
            }
        }

        // Clear buffer as all data has been processed
        self.buffer.clear();

        Ok(self.parser.screen().contents())
    }
}
