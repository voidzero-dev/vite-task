use std::{hash::Hasher as _, io};

use twox_hash::XxHash3_64;
use vt_path::AbsolutePath;

use super::redact::{UnexpectedFormat, pnpm_modules_manifest};

/// Hash content using 8 KiB buffered `xxHash3_64`.
pub(super) fn hash_content(mut stream: impl io::Read) -> io::Result<u64> {
    let mut hasher = XxHash3_64::default();
    let mut buf = [0u8; 8192];
    loop {
        let n = stream.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.write(&buf[..n]);
    }
    Ok(hasher.finish())
}

/// Hash the content of the file at `path`, read from `stream`. Files with
/// noise in their content are hashed without it; see [`super::redact`].
pub(super) fn hash_file_content(path: &AbsolutePath, stream: impl io::Read) -> io::Result<u64> {
    if pnpm_modules_manifest::matches(path) {
        return hash_redacted(stream, pnpm_modules_manifest::redact);
    }
    hash_content(stream)
}

/// Hash what `redact` writes for the content of `stream`, or the raw content
/// when it is not in the format `redact` expects.
fn hash_redacted(
    mut stream: impl io::Read,
    redact: impl FnOnce(&[u8], &mut HashWriter) -> Result<(), UnexpectedFormat>,
) -> io::Result<u64> {
    let mut content = Vec::new();
    stream.read_to_end(&mut content)?;
    let mut hasher = HashWriter::default();
    match redact(&content, &mut hasher) {
        Ok(()) => Ok(hasher.finish()),
        Err(UnexpectedFormat) => hash_content(content.as_slice()),
    }
}

/// Hashes the bytes written to it with `xxHash3_64`.
#[derive(Default)]
struct HashWriter(XxHash3_64);

impl HashWriter {
    fn finish(&self) -> u64 {
        self.0.finish()
    }
}

impl io::Write for HashWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &[u8] =
        br#"{ "packageManager": "pnpm@11.24.0", "prunedAt": "Thu, 08 Oct 2026 08:18:18 GMT" }"#;
    const MANIFEST_REINSTALLED: &[u8] =
        br#"{ "packageManager": "pnpm@11.24.0", "prunedAt": "Fri, 09 Oct 2026 03:13:58 GMT" }"#;

    fn hash_at(relative_path: &str, content: &[u8]) -> u64 {
        let path = vt_path::current_dir().unwrap().join(relative_path);
        hash_file_content(&path, content).unwrap()
    }

    #[test]
    fn redacted_file_hashes_without_noise() {
        assert_eq!(
            hash_at("node_modules/.modules.yaml", MANIFEST),
            hash_at("node_modules/.modules.yaml", MANIFEST_REINSTALLED)
        );
    }

    #[test]
    fn unparsable_redacted_file_hashes_raw_content() {
        let content = b"{ \"prunedAt\": ";
        assert_eq!(
            hash_at("node_modules/.modules.yaml", content),
            hash_content(&content[..]).unwrap()
        );
    }

    #[test]
    fn other_files_hash_raw_content() {
        assert_eq!(hash_at(".modules.yaml", MANIFEST), hash_content(MANIFEST).unwrap());
    }
}
