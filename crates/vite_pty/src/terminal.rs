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

    /// Unprocessed data buffer for read_until
    read_until_buffer: Vec<u8>,
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
            read_until_buffer: Vec::new(),
        })
    }

    /// Read until the first occurrence of the expected string is found.
    /// Multiple occurrences may be buffered internally. Keep calling with the same string to
    /// find subsequent occurrences.
    ///
    /// However, `screen_contents` will reflect all data, including subsequent occurrences,
    /// even before they are consumed by `read_until`. It is designed this way because the
    /// screen must always have latest data for proper query responses.
    pub fn read_until(&mut self, expected: &str) -> anyhow::Result<()> {
        let expected_bytes = expected.as_bytes();

        let mut buf = [0u8; 8192];

        loop {
            // look for the expected str in buffer
            // There could be buffered occurrences in the first iteration,
            // or new data read from the previous iteration.
            if let Some(pos) = self
                .read_until_buffer
                .windows(expected_bytes.len())
                .position(|window| window == expected_bytes)
            {
                // Consume data in read_until_buffer before and including the expected str
                let split_pos = pos + expected_bytes.len();
                self.read_until_buffer.drain(0..split_pos);
                return Ok(());
            }

            // Not found yet - read more data
            let n = self.reader.read(&mut buf)?;

            let data = &buf[..n];
            // Feed data to parser, which updates screen state and handles control sequence queries.
            self.parser.process(data);

            self.read_until_buffer.extend_from_slice(data);
        }
    }

    pub fn kill(&mut self) -> anyhow::Result<()> {
        self.child_killer.kill()?;
        Ok(())
    }

    pub fn read_to_end(&mut self) -> anyhow::Result<String> {
        // `read_to_end` will move cursor to the end, so clear any buffered data for `read_until`
        self.read_until_buffer.clear();

        let mut buf = [0u8; 8192];
        // Read all remaining data until EOF
        loop {
            let n = self.reader.read(&mut buf)?;
            self.parser.process(&buf[..n]);
            if n == 0 {
                break;
            }
        }
        Ok(self.screen_contents())
    }

    pub fn screen_contents(&self) -> String {
        self.parser.screen().contents()
    }
}
