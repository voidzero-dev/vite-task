use std::{hash::Hasher as _, io};

use twox_hash::XxHash3_64;
use vt_path::AbsolutePath;

use super::redact::{node_modules_bin, pnpm_modules_manifest};

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

/// Hash the content of the file at `path`, read from `stream`. Files whose
/// content is noise hash as empty, so only whether they exist is
/// fingerprinted; see [`super::redact`].
pub(super) fn hash_file_content(path: &AbsolutePath, stream: impl io::Read) -> io::Result<u64> {
    if pnpm_modules_manifest::matches(path) || node_modules_bin::matches(path) {
        return hash_content(io::empty());
    }
    hash_content(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &[u8] =
        br#"{ "packageManager": "pnpm@11.24.0", "prunedAt": "Thu, 08 Oct 2026 08:18:18 GMT" }"#;
    const MANIFEST_REINSTALLED: &[u8] =
        br#"{ "packageManager": "pnpm@11.25.0", "prunedAt": "Fri, 09 Oct 2026 03:13:58 GMT" }"#;

    fn hash_at(relative_path: &str, content: &[u8]) -> u64 {
        let path = vt_path::current_dir().unwrap().join(relative_path);
        hash_file_content(&path, content).unwrap()
    }

    #[test]
    fn redacted_file_hashes_without_content() {
        assert_eq!(
            hash_at("node_modules/.modules.yaml", MANIFEST),
            hash_at("node_modules/.modules.yaml", MANIFEST_REINSTALLED)
        );
    }

    #[test]
    fn other_files_hash_raw_content() {
        assert_eq!(hash_at(".modules.yaml", MANIFEST), hash_content(MANIFEST).unwrap());
    }
}
