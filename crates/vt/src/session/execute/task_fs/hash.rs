use std::{hash::Hasher as _, io};

/// Hash content using 8 KiB buffered `xxHash3_64`.
pub(super) fn hash_content(mut stream: impl io::Read) -> io::Result<u64> {
    let mut hasher = twox_hash::XxHash3_64::default();
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
