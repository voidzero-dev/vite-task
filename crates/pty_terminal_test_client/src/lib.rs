use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};

const MILESTONE_TITLE_MARKER: &str = "pty-terminal-test:";

#[cfg(any(feature = "testing", test))]
fn encode_milestone_title(name: &str) -> String {
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).expect("failed to generate milestone identity");
    let id = u128::from_be_bytes(random);
    let encoded_name = URL_SAFE_NO_PAD.encode(name.as_bytes());
    format!("{MILESTONE_TITLE_MARKER}{id:032x}:{encoded_name}")
}

/// Decodes a milestone title, ignoring ordinary application title updates.
#[must_use]
pub fn decode_milestone_title(title: &[u8]) -> Option<String> {
    let encoded = title.strip_prefix(MILESTONE_TITLE_MARKER.as_bytes())?;
    let (encoded_id, encoded_name) = encoded.split_at_checked(32)?;
    let (&b':', encoded_name) = encoded_name.split_first()? else {
        return None;
    };
    if !encoded_id.iter().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte)) {
        return None;
    }

    let name_bytes = URL_SAFE_NO_PAD.decode(encoded_name).ok()?;
    if URL_SAFE_NO_PAD.encode(&name_bytes).as_bytes() != encoded_name {
        return None;
    }
    String::from_utf8(name_bytes).ok()
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
/// Panics if secure randomness is unavailable or emitting the title fails.
#[cfg(feature = "testing")]
pub fn mark_milestone(name: &str) {
    emit_title(&encode_milestone_title(name)).expect("failed to emit milestone title");
}

#[cfg(all(feature = "testing", windows))]
fn emit_title(title: &str) -> std::io::Result<()> {
    use std::io::Write as _;

    std::io::stdout().flush()?;
    let mut wide = title.encode_utf16().collect::<Vec<_>>();
    wide.push(0);
    // SAFETY: `wide` is a valid NUL-terminated UTF-16 title.
    if unsafe { winapi::um::wincon::SetConsoleTitleW(wide.as_ptr()) } == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(all(feature = "testing", not(windows)))]
fn emit_title(title: &str) -> std::io::Result<()> {
    use std::io::Write as _;

    let mut stdout = std::io::stdout().lock();
    stdout.flush()?;
    write!(stdout, "\x1b]2;{title}\x1b\\")?;
    stdout.flush()
}

/// Does nothing when milestone instrumentation is disabled.
///
/// When the `testing` feature is disabled, this is a no-op.
#[cfg(not(feature = "testing"))]
pub const fn mark_milestone(_name: &str) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_round_trip() {
        let title = encode_milestone_title("task-select:lib#:0");
        assert_eq!(decode_milestone_title(title.as_bytes()).as_deref(), Some("task-select:lib#:0"));
    }

    #[test]
    fn repeated_names_get_unique_titles() {
        assert_ne!(encode_milestone_title("ready"), encode_milestone_title("ready"));
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
