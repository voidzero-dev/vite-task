//! A [`Write`] wrapper that prefixes each line with a label (e.g., `[pkg#task] `).

use std::io::{self, Write};

/// Writer that prefixes each complete line with a label.
///
/// Data is buffered internally. On [`flush`](Write::flush), complete lines
/// (terminated by `\n`) are written to the inner writer with the prefix
/// prepended. Any trailing partial line is kept in the buffer until the
/// next flush.
pub struct LabeledWriter {
    inner: Box<dyn Write>,
    prefix: Vec<u8>,
    buffer: Vec<u8>,
}

impl LabeledWriter {
    pub fn new(inner: Box<dyn Write>, prefix: Vec<u8>) -> Self {
        Self { inner, prefix, buffer: Vec::new() }
    }
}

impl Write for LabeledWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        // Find the last newline — everything up to (and including) it can be
        // split into complete lines and written with prefixes.
        let last_nl = self.buffer.iter().rposition(|&b| b == b'\n');
        let Some(last_nl) = last_nl else {
            // No complete lines yet — keep buffering.
            return Ok(());
        };

        // Split off the complete portion (0..=last_nl) from any trailing partial line.
        let remaining = self.buffer.split_off(last_nl + 1);
        let complete = std::mem::replace(&mut self.buffer, remaining);

        // Batch prefix + line into a single write to reduce syscall overhead.
        let mut prefixed = Vec::new();
        for line in complete.split_inclusive(|&b| b == b'\n') {
            prefixed.extend_from_slice(&self.prefix);
            prefixed.extend_from_slice(line);
        }
        self.inner.write_all(&prefixed)?;

        self.inner.flush()
    }
}

impl Drop for LabeledWriter {
    fn drop(&mut self) {
        // Flush any remaining partial line on drop.
        if !self.buffer.is_empty() {
            let buf = std::mem::take(&mut self.buffer);
            let _ = self.inner.write_all(&self.prefix);
            let _ = self.inner.write_all(&buf);
            let _ = self.inner.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labeled_output(prefix: &str, chunks: &[&[u8]]) -> Vec<u8> {
        let output = Vec::new();
        let mut writer = LabeledWriter::new(Box::new(output), prefix.as_bytes().to_vec());
        for chunk in chunks {
            writer.write_all(chunk).unwrap();
            writer.flush().unwrap();
        }
        drop(writer);
        // We need to get the inner writer back — reconstruct by running again
        // and capturing output via a shared buffer.
        let output = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let output_clone = std::rc::Rc::clone(&output);
        {
            let mut writer =
                LabeledWriter::new(Box::new(RcWriter(output_clone)), prefix.as_bytes().to_vec());
            for chunk in chunks {
                writer.write_all(chunk).unwrap();
                writer.flush().unwrap();
            }
        }
        std::rc::Rc::try_unwrap(output).unwrap().into_inner()
    }

    struct RcWriter(std::rc::Rc<std::cell::RefCell<Vec<u8>>>);

    impl Write for RcWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// (case, prefix, chunks written and flushed in order, expected output)
    type LabeledCase<'a> = (&'a str, &'a str, &'a [&'a [u8]], &'a [u8]);

    #[test]
    fn prefixes_each_line() {
        let cases: &[LabeledCase<'_>] = &[
            ("single complete line", "[app] ", &[b"hello\n"], b"[app] hello\n"),
            (
                "multiple lines in one chunk",
                "[a] ",
                &[b"line1\nline2\n"],
                b"[a] line1\n[a] line2\n",
            ),
            ("partial line flushed on drop", "[x] ", &[b"no newline"], b"[x] no newline"),
            ("split across chunks", "[p] ", &[b"split ", b"line\n"], b"[p] split line\n"),
            ("empty line", "[e] ", &[b"\n"], b"[e] \n"),
            ("multiple flushes with partial", "[m] ", &[b"a\nb", b"c\n"], b"[m] a\n[m] bc\n"),
        ];
        for &(case, prefix, chunks, expected) in cases {
            assert_eq!(labeled_output(prefix, chunks), expected, "{case}");
        }
    }
}
