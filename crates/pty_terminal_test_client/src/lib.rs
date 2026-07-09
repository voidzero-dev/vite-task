use std::io;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};

/// Prefix distinguishing milestone titles from ordinary application titles.
pub const MILESTONE_TITLE_MARKER: &str = "pty-terminal-test:";
const MAX_MILESTONE_NAME_BYTES: usize = 144;

/// A decoded window-title milestone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedMilestone {
    /// Random occurrence identity.
    pub id: u128,
    /// Caller-provided milestone name.
    pub name: String,
}

/// Builds a unique title token for a milestone occurrence.
///
/// # Errors
///
/// Returns an error when the name is empty or too long, secure randomness is
/// unavailable, or the generated title exceeds the Windows title limit.
pub fn encode_milestone_title(name: &str) -> io::Result<String> {
    if name.is_empty() || name.len() > MAX_MILESTONE_NAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "milestone name must contain 1 to 144 UTF-8 bytes",
        ));
    }

    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(io::Error::other)?;
    let id = u128::from_be_bytes(random);
    let encoded_name = URL_SAFE_NO_PAD.encode(name.as_bytes());
    let title = format!("{MILESTONE_TITLE_MARKER}{id:032x}:{encoded_name}");
    if title.len() >= 255 {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "milestone title is too long"));
    }
    Ok(title)
}

/// Decodes a milestone title, ignoring ordinary application title updates.
#[must_use]
pub fn decode_milestone_title(title: &[u8]) -> Option<DecodedMilestone> {
    let encoded = title.strip_prefix(MILESTONE_TITLE_MARKER.as_bytes())?;
    let (encoded_id, encoded_name) = encoded.split_at_checked(32)?;
    let (&b':', encoded_name) = encoded_name.split_first()? else {
        return None;
    };
    if !encoded_id.iter().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
        || encoded_name.is_empty()
    {
        return None;
    }

    let id = u128::from_str_radix(std::str::from_utf8(encoded_id).ok()?, 16).ok()?;
    let name_bytes = URL_SAFE_NO_PAD.decode(encoded_name).ok()?;
    if name_bytes.is_empty()
        || name_bytes.len() > MAX_MILESTONE_NAME_BYTES
        || URL_SAFE_NO_PAD.encode(&name_bytes).as_bytes() != encoded_name
    {
        return None;
    }
    Some(DecodedMilestone { id, name: String::from_utf8(name_bytes).ok()? })
}

/// Emits a milestone marker as a unique window-title update.
///
/// The child process calls this to signal it has reached a named synchronization
/// point. The test harness (via `pty_terminal_test::Reader::expect_milestone`)
/// detects this marker and returns the screen contents at that point.
///
/// Windows uses `SetConsoleTitleW`, which `ConPTY` emits through its renderer after
/// preceding text and cursor state. Other platforms emit the equivalent OSC 2
/// title update through the ordered PTY byte stream.
///
/// When the `testing` feature is disabled, this is a no-op.
///
/// # Panics
///
/// Panics if writing to stdout fails.
#[cfg(feature = "testing")]
pub fn mark_milestone(name: &str) {
    try_mark_milestone(name).expect("failed to emit milestone title");
}

/// Tries to emit a milestone title.
///
/// # Errors
///
/// Returns an error if title encoding, output flushing, or the platform title
/// operation fails.
#[cfg(feature = "testing")]
pub fn try_mark_milestone(name: &str) -> io::Result<()> {
    emit_title(&encode_milestone_title(name)?)
}

#[cfg(all(feature = "testing", windows))]
fn emit_title(title: &str) -> io::Result<()> {
    use std::io::Write as _;

    std::io::stdout().flush()?;
    let mut wide = title.encode_utf16().collect::<Vec<_>>();
    wide.push(0);
    // SAFETY: `wide` is a valid NUL-terminated UTF-16 title.
    if unsafe { winapi::um::wincon::SetConsoleTitleW(wide.as_ptr()) } == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(all(feature = "testing", not(windows)))]
fn emit_title(title: &str) -> io::Result<()> {
    use std::io::Write as _;

    let mut stdout = std::io::stdout().lock();
    stdout.flush()?;
    write!(stdout, "\x1b]2;{title}\x1b\\")?;
    stdout.flush()
}

/// Emits a milestone marker as a private OSC escape sequence.
///
/// When the `testing` feature is disabled, this is a no-op.
#[cfg(not(feature = "testing"))]
pub const fn mark_milestone(_name: &str) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_round_trip() {
        let title = encode_milestone_title("task-select:lib#:0").unwrap();
        let decoded = decode_milestone_title(title.as_bytes()).unwrap();
        assert_eq!(decoded.name, "task-select:lib#:0");
    }

    #[test]
    fn repeated_names_get_unique_ids() {
        let first = encode_milestone_title("ready").unwrap();
        let second = encode_milestone_title("ready").unwrap();
        assert_ne!(
            decode_milestone_title(first.as_bytes()).unwrap().id,
            decode_milestone_title(second.as_bytes()).unwrap().id
        );
    }

    #[test]
    fn ignores_normal_and_malformed_titles() {
        assert!(decode_milestone_title(b"normal title").is_none());
        assert!(decode_milestone_title(b"pty-terminal-test:not-hex:cmVhZHk").is_none());
        assert!(
            decode_milestone_title(b"pty-terminal-test:00000000000000000000000000000000:*")
                .is_none()
        );
    }
}
