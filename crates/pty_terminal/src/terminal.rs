use std::{
    io::{Read, Write},
    sync::{Arc, Mutex, OnceLock},
    thread,
};

pub use portable_pty::CommandBuilder;
use portable_pty::{ChildKiller, ExitStatus, MasterPty};

use crate::geo::ScreenSize;

/// A headless terminal
pub struct Terminal {
    master: Box<dyn MasterPty + Send>,
    parser: vt100::Parser<Vt100Callbacks>,
    child_killer: Box<dyn ChildKiller + Send + Sync>,
    reader: Box<dyn Read + Send>,
    writer: Arc<Mutex<Option<Box<dyn Write + Send>>>>,

    /// Unprocessed data buffer for `read_until`
    read_until_buffer: Vec<u8>,

    /// Exit status from the child process, set once by background thread
    exit_status: Arc<OnceLock<ExitStatus>>,
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
    /// Spawns a new child process in a headless terminal with the given size and command.
    ///
    /// # Errors
    ///
    /// Returns an error if the PTY cannot be opened or the command fails to spawn.
    ///
    /// # Panics
    ///
    /// Panics if the writer lock is poisoned when the background thread closes it.
    pub fn spawn(size: ScreenSize, cmd: CommandBuilder) -> anyhow::Result<Self> {
        let pty_pair = portable_pty::native_pty_system().openpty(portable_pty::PtySize {
            rows: size.rows,
            cols: size.cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        // Create reader BEFORE spawning child to ensure it's ready for data
        let reader = pty_pair.master.try_clone_reader()?;
        let writer: Arc<Mutex<Option<Box<dyn Write + Send>>>> =
            Arc::new(Mutex::new(Some(pty_pair.master.take_writer()?)));
        // Spawn child and immediately drop slave to ensure EOF is signaled when child exits
        let mut child = pty_pair.slave.spawn_command(cmd)?;
        let child_killer = child.clone_killer();
        drop(pty_pair.slave); // Critical: drop slave so EOF is signaled when child exits
        let master = pty_pair.master;
        let exit_status: Arc<OnceLock<ExitStatus>> = Arc::new(OnceLock::new());

        // Background thread: wait for child to exit, set exit status, then close writer to trigger EOF
        thread::spawn({
            let writer = Arc::clone(&writer);
            let exit_status = Arc::clone(&exit_status);
            move || {
                // Wait for child and set exit status
                if let Ok(status) = child.wait() {
                    let _ = exit_status.set(status);
                }
                // Close writer to signal EOF to the reader
                *writer.lock().unwrap() = None;
            }
        });

        Ok(Self {
            master,
            parser: vt100::Parser::new_with_callbacks(
                size.rows,
                size.cols,
                0,
                Vt100Callbacks { writer: Arc::clone(&writer) },
            ),
            child_killer,
            reader,
            read_until_buffer: Vec::new(),
            writer,
            exit_status,
        })
    }

    /// Read until the first occurrence of the expected string is found.
    /// Multiple occurrences may be buffered internally. Keep calling with the same string to
    /// find subsequent occurrences.
    ///
    /// However, `screen_contents` will reflect all data, including subsequent occurrences,
    /// even before they are consumed by `read_until`. It is designed this way because the
    /// screen must always have latest data for proper query responses.
    ///
    /// # Errors
    ///
    /// Returns an error if the expected string is not found before EOF or if reading fails.
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

            if n == 0 {
                // EOF - expected string not found
                return Err(anyhow::anyhow!("Expected string not found: {expected}"));
            }

            let data = &buf[..n];
            // Feed data to parser, which updates screen state and handles control sequence queries.
            self.parser.process(data);

            self.read_until_buffer.extend_from_slice(data);
        }
    }

    /// Kills the child process.
    ///
    /// # Errors
    ///
    /// Returns an error if the child process cannot be killed.
    pub fn kill(&mut self) -> anyhow::Result<()> {
        self.child_killer.kill()?;
        Ok(())
    }

    /// Reads all remaining output until the child process exits.
    ///
    /// Returns the exit status of the child process.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Reading from the PTY fails
    /// - The exit status is not available (should not happen in normal operation)
    ///
    /// # Panics
    ///
    /// Panics if the writer lock is poisoned.
    pub fn read_to_end(&mut self) -> anyhow::Result<ExitStatus> {
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

        // Wait for exit status to be set by background thread
        let status = self.exit_status.wait().clone();

        // Close the writer since the child has exited and all output has been consumed.
        // This ensures subsequent write() calls fail immediately, rather than racing
        // with the background thread which also closes the writer.
        *self.writer.lock().unwrap() = None;

        Ok(status)
    }

    /// Writes data to the child process's stdin.
    ///
    /// # Errors
    ///
    /// Returns an error if the child process has already exited or if writing fails.
    pub fn write(&self, data: &[u8]) -> anyhow::Result<()> {
        // On Windows ConPTY, convert LF to CRLF for proper line handling
        #[cfg(target_os = "windows")]
        let converted: Vec<u8> = {
            let mut result = Vec::new();
            for &byte in data {
                if byte == b'\n' {
                    result.push(b'\r');
                    result.push(b'\n');
                } else {
                    result.push(byte);
                }
            }
            result
        };

        #[cfg(target_os = "windows")]
        let data_to_write: &[u8] = &converted;

        #[cfg(not(target_os = "windows"))]
        let data_to_write: &[u8] = data;

        let mut writer_guard = self
            .writer
            .lock()
            .map_err(|e| anyhow::anyhow!("Failed to acquire writer lock: {e}"))?;

        if let Some(writer) = writer_guard.as_mut() {
            writer.write_all(data_to_write)?;
            writer.flush()?;
            Ok(())
        } else {
            Err(anyhow::anyhow!("Cannot write: child process has exited"))
        }
    }

    /// Sends Ctrl+C (SIGINT) to the child process.
    ///
    /// Writes ETX (0x03) to the PTY. On Unix, the terminal driver converts this
    /// to SIGINT for the child's process group. On Windows, `ConPTY` intercepts
    /// the byte and generates `CTRL_C_EVENT` for the child.
    ///
    /// # Errors
    ///
    /// Returns an error if the child process has already exited or writing fails.
    pub fn send_ctrl_c(&self) -> anyhow::Result<()> {
        self.write(&[0x03])
    }

    /// Clones the child process killer for use from another thread.
    #[must_use]
    pub fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
        self.child_killer.clone_killer()
    }

    #[must_use]
    pub fn screen_contents(&self) -> String {
        self.parser.screen().contents()
    }

    /// Resizes the terminal to the given size.
    ///
    /// # Errors
    ///
    /// Returns an error if the PTY cannot be resized.
    pub fn resize(&mut self, size: ScreenSize) -> anyhow::Result<()> {
        // Resize the underlying PTY via portable-pty's MasterPty::resize
        self.master.resize(portable_pty::PtySize {
            rows: size.rows,
            cols: size.cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        // Update the vt100 parser's internal screen dimensions
        self.parser.screen_mut().set_size(size.rows, size.cols);

        Ok(())
    }
}
