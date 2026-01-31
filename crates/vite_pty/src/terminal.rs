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

    /// Read data into the internal buffer `self.buffer`
    /// Returns the number of new bytes added to buffer. If EOF is reached, returns 0.
    fn read_to_buffer(&mut self) -> anyhow::Result<usize> {
        let mut buffer = [0u8; 4096];
        let n = self.reader.read(&mut buffer)?;

        if n == 0 {
            return Ok(0); // EOF
        }

        self.buffer.extend_from_slice(&buffer[..n]);
        Ok(n)
    }

    /// Consume `n` bytes from the internal buffer, processing them through the parser.
    fn consume(&mut self, n: usize) -> anyhow::Result<()> {
        if n > self.buffer.len() {
            return Err(anyhow::anyhow!(
                "Cannot consume {} bytes, only {} available",
                n,
                self.buffer.len()
            ));
        }

        // Process first n bytes through parser (important for Windows)
        self.parser.process(&self.buffer[..n]);

        // Remove first n bytes from buffer
        self.buffer = self.buffer[n..].to_vec();

        Ok(())
    }

    pub fn read_until(&mut self, expected: &str) -> anyhow::Result<()> {
        let expected_bytes = expected.as_bytes();

        loop {
            // Check buffer before reading if it has data (from previous iteration)
            if !self.buffer.is_empty() {
                if let Some(pos) = self
                    .buffer
                    .windows(expected_bytes.len())
                    .position(|window| window == expected_bytes)
                {
                    let split_pos = pos + expected_bytes.len();
                    self.consume(split_pos)?;
                    return Ok(());
                }
            }

            // 1. read_to_buffer
            let n = self.read_to_buffer()?;

            // 2. look for the expected str in buffer (after reading)
            if let Some(pos) = self
                .buffer
                .windows(expected_bytes.len())
                .position(|window| window == expected_bytes)
            {
                // 3. Found: consume data before and including the expected str, then return
                let split_pos = pos + expected_bytes.len();
                self.consume(split_pos)?;
                return Ok(());
            }

            if n == 0 {
                // EOF - consume any remaining buffer before returning error
                if !self.buffer.is_empty() {
                    let buffer_len = self.buffer.len();
                    self.consume(buffer_len)?;
                }
                return Err(anyhow::anyhow!("Expected string not found: {}", expected));
            }

            // 4. Not found: check how much of the buffer end is a prefix of expected
            // Keep that tail, consume the rest
            let consume_amount = if self.buffer.len() >= expected_bytes.len() {
                // Buffer is large enough to contain the full expected string but doesn't
                // Consume everything to make progress
                self.buffer.len()
            } else {
                // Buffer is smaller - check for prefix match for boundary spanning
                let mut keep_len = 0;
                for len in (1..=self.buffer.len()).rev() {
                    if &self.buffer[self.buffer.len() - len..] == &expected_bytes[..len] {
                        keep_len = len;
                        break;
                    }
                }
                self.buffer.len() - keep_len
            };

            if consume_amount > 0 {
                self.consume(consume_amount)?;
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
            let n = self.read_to_buffer()?;
            if n == 0 {
                break;
            }

            // Consume all buffered data (process and remove)
            let buffer_len = self.buffer.len();
            self.consume(buffer_len)?;
        }

        // Consume any remaining buffer after EOF
        if !self.buffer.is_empty() {
            let buffer_len = self.buffer.len();
            self.consume(buffer_len)?;
        }

        Ok(self.parser.screen().contents())
    }
}
