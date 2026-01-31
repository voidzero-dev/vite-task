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
        })
    }

    /// Read until the expected string is found in the terminal output.
    pub fn read_until(&mut self, expected: &str) -> anyhow::Result<()> {
        let mut reader = self.pty_pair.master.try_clone_reader()?;
        let mut buffer = [0u8; 4096];
        let mut collected = Vec::<u8>::new();
        loop {
            let n = reader.read(&mut buffer)?;
            if n == 0 {
                return Err(anyhow::anyhow!("Expected string not found: {}", expected));
            }
            let data = &buffer[..n];
            self.parser.process(data);
            collected.extend_from_slice(&data);

            if collected
                .windows(expected.as_bytes().len())
                .any(|window| window == expected.as_bytes())
            {
                return Ok(());
            }
        }
    }

    pub fn kill(&mut self) -> anyhow::Result<()> {
        self.child_killer.kill()?;
        Ok(())
    }

    pub fn read_to_end(&mut self) -> anyhow::Result<String> {
        let mut reader = self.pty_pair.master.try_clone_reader()?;
        let mut buffer = [0u8; 4096];

        loop {
            let n = reader.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            self.parser.process(&buffer[..n]);
        }

        Ok(self.parser.screen().contents())
    }
}
