//! Cross-platform sentinel probe paths for detecting an active outer `fspy`
//! tracker.
//!
//! Each platform's `fspy` probes a fixed, random-GUID path located underneath
//! the OS null device (`/dev/null/...` on Unix, `\??\NUL\...` on Windows).
//! Because the null device is a character device rather than a directory, the
//! kernel always rejects lookups beneath it. Only an active outer `fspy`
//! preload intercepts the exact sentinel path and short-circuits the syscall
//! to success without recording it as a file access.

use core::ffi::CStr;

/// NUL-terminated Unix sentinel path used by `fspy` on Linux and macOS to
/// detect an active outer `fspy` tracker via `faccessat(AT_FDCWD, path, F_OK)`.
pub const NESTED_PROBE_PATH_UNIX: &CStr =
    c"/dev/null/fspy_probe_92e41940-369e-42f1-876f-a8e2a614fbf2";

/// Byte slice (excluding trailing NUL) of [`NESTED_PROBE_PATH_UNIX`].
pub const NESTED_PROBE_PATH_UNIX_BYTES: &[u8] = NESTED_PROBE_PATH_UNIX.to_bytes();

const NESTED_PROBE_PATH_WINDOWS_ASCII: &[u8] =
    br"\??\NUL\fspy_probe_92e41940-369e-42f1-876f-a8e2a614fbf2";

const WINDOWS_PROBE_LEN: usize = NESTED_PROBE_PATH_WINDOWS_ASCII.len();

const fn ascii_to_utf16<const N: usize>(bytes: &[u8]) -> [u16; N] {
    assert!(bytes.len() == N);
    let mut out = [0u16; N];
    let mut i = 0;
    while i < N {
        assert!(bytes[i].is_ascii());
        out[i] = bytes[i] as u16;
        i += 1;
    }
    out
}

const fn ascii_to_utf16_nul<const N: usize, const M: usize>(bytes: &[u8]) -> [u16; M] {
    assert!(bytes.len() == N);
    assert!(M == N + 1);
    let mut out = [0u16; M];
    let mut i = 0;
    while i < N {
        assert!(bytes[i].is_ascii());
        out[i] = bytes[i] as u16;
        i += 1;
    }
    out[N] = 0;
    out
}

const NESTED_PROBE_PATH_WINDOWS_UTF16_ARRAY: [u16; WINDOWS_PROBE_LEN] =
    ascii_to_utf16::<WINDOWS_PROBE_LEN>(NESTED_PROBE_PATH_WINDOWS_ASCII);

/// UTF-16 slice (excluding trailing NUL) of the Windows NT object sentinel
/// path `\??\NUL\fspy_probe_92e41940-369e-42f1-876f-a8e2a614fbf2`.
pub const NESTED_PROBE_PATH_WINDOWS_UTF16: &[u16] = &NESTED_PROBE_PATH_WINDOWS_UTF16_ARRAY;

/// NUL-terminated UTF-16 static buffer of the Windows NT object sentinel path,
/// suitable for populating a `UNICODE_STRING.Buffer` in `NtQueryAttributesFile`.
pub static NESTED_PROBE_PATH_WINDOWS_UTF16_NUL: [u16; WINDOWS_PROBE_LEN + 1] =
    ascii_to_utf16_nul::<WINDOWS_PROBE_LEN, { WINDOWS_PROBE_LEN + 1 }>(
        NESTED_PROBE_PATH_WINDOWS_ASCII,
    );

/// Byte length (excluding trailing NUL) for `UNICODE_STRING.Length`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "constant ASCII Windows sentinel path fits well within u16"
)]
pub const NESTED_PROBE_PATH_WINDOWS_BYTE_LEN: u16 = (WINDOWS_PROBE_LEN * 2) as u16;

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;

    #[test]
    fn unix_sentinel_path_is_under_dev_null() {
        assert!(NESTED_PROBE_PATH_UNIX_BYTES.starts_with(b"/dev/null/fspy_probe_"));
        assert_eq!(NESTED_PROBE_PATH_UNIX.to_bytes(), NESTED_PROBE_PATH_UNIX_BYTES);
    }

    #[test]
    fn windows_sentinel_path_utf16_matches_ascii() {
        let expected: Vec<u16> =
            NESTED_PROBE_PATH_WINDOWS_ASCII.iter().map(|&b| u16::from(b)).collect();
        assert_eq!(NESTED_PROBE_PATH_WINDOWS_UTF16, expected.as_slice());
        assert_eq!(
            usize::from(NESTED_PROBE_PATH_WINDOWS_BYTE_LEN),
            NESTED_PROBE_PATH_WINDOWS_UTF16.len() * 2
        );
        assert_eq!(
            &NESTED_PROBE_PATH_WINDOWS_UTF16_NUL[..NESTED_PROBE_PATH_WINDOWS_UTF16.len()],
            NESTED_PROBE_PATH_WINDOWS_UTF16
        );
        assert_eq!(*NESTED_PROBE_PATH_WINDOWS_UTF16_NUL.last().unwrap(), 0);
    }
}
