use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};

const MILESTONE_TITLE_MARKER: &str = "vt-pty-terminal-test:";

/// Environment variable through which the test harness asks milestone
/// producers on Windows to wait until it has seen each milestone. Its value
/// identifies the test terminal.
pub const MILESTONE_LOCK_ENV: &str = "VT_PTY_TERMINAL_TEST_MILESTONE_LOCK";

/// A milestone decoded from a window title.
#[derive(Debug, PartialEq, Eq)]
pub struct Milestone {
    /// Random identity that distinguishes repeated milestones with the same name.
    pub id: u128,
    pub name: String,
}

/// Name of the Windows event that the producer of milestone `id` waits on
/// until the test harness has seen its title.
#[must_use]
pub fn milestone_ack_event_name(id: u128) -> String {
    format!("Local\\vt-pty-terminal-test-ack-{id:032x}")
}

#[cfg(any(feature = "testing", test))]
fn new_milestone_id() -> u128 {
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).expect("failed to generate milestone identity");
    u128::from_be_bytes(random)
}

#[cfg(any(feature = "testing", test))]
fn encode_milestone_title(id: u128, name: &str) -> String {
    let encoded_name = URL_SAFE_NO_PAD.encode(name.as_bytes());
    format!("{MILESTONE_TITLE_MARKER}{id:032x}:{encoded_name}")
}

/// Decodes a milestone title, ignoring ordinary application title updates.
#[must_use]
pub fn decode_milestone_title(title: &[u8]) -> Option<Milestone> {
    let encoded = title.strip_prefix(MILESTONE_TITLE_MARKER.as_bytes())?;
    let (encoded_id, encoded_name) = encoded.split_at_checked(32)?;
    let (&b':', encoded_name) = encoded_name.split_first()? else {
        return None;
    };
    if !encoded_id.iter().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte)) {
        return None;
    }

    let id = u128::from_str_radix(std::str::from_utf8(encoded_id).ok()?, 16).ok()?;

    let name_bytes = URL_SAFE_NO_PAD.decode(encoded_name).ok()?;
    if URL_SAFE_NO_PAD.encode(&name_bytes).as_bytes() != encoded_name {
        return None;
    }
    Some(Milestone { id, name: String::from_utf8(name_bytes).ok()? })
}

/// Emits a milestone marker as a unique window-title update.
///
/// The child process calls this to signal it has reached a named synchronization
/// point. The test harness (via `vt_pty_terminal_test::Reader::expect_milestone`)
/// detects this marker and returns the screen contents at that point.
///
/// Windows uses `SetConsoleTitleW`, which `ConPTY` emits through its renderer after
/// preceding text and cursor state. Other platforms emit the equivalent OSC 2
/// title update through the ordered PTY byte stream.
///
/// `ConPTY` emits only the latest title when it renders, so on Windows a title
/// set by another process before then would replace the milestone. When
/// [`MILESTONE_LOCK_ENV`] is set, this holds a lock named after it, shared by
/// the processes in the test terminal, until the harness signals the event
/// named by [`milestone_ack_event_name`] after it has seen the title.
///
/// When the `testing` feature is disabled, this is a no-op.
///
/// # Panics
///
/// Panics if secure randomness is unavailable or emitting the title fails.
#[cfg(feature = "testing")]
pub fn mark_milestone(name: &str) {
    let id = new_milestone_id();
    emit_title(id, &encode_milestone_title(id, name)).expect("failed to emit milestone title");
}

#[cfg(all(feature = "testing", windows))]
fn emit_title(id: u128, title: &str) -> std::io::Result<()> {
    use std::{
        io::Write as _,
        os::windows::io::{FromRawHandle as _, OwnedHandle},
        ptr,
    };

    use winapi::um::{synchapi, winbase, wincon};

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    fn check_handle(handle: winapi::um::winnt::HANDLE) -> std::io::Result<OwnedHandle> {
        if handle.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `handle` is a valid handle that nothing else owns.
        Ok(unsafe { OwnedHandle::from_raw_handle(handle.cast()) })
    }

    fn wait(handle: &OwnedHandle) -> std::io::Result<()> {
        use std::os::windows::io::AsRawHandle as _;

        // SAFETY: `handle` is a valid synchronization object.
        match unsafe {
            synchapi::WaitForSingleObject(handle.as_raw_handle().cast(), winbase::INFINITE)
        } {
            // A lock abandoned by a process that exited while holding it is
            // still acquired.
            winbase::WAIT_OBJECT_0 | winbase::WAIT_ABANDONED => Ok(()),
            _ => Err(std::io::Error::last_os_error()),
        }
    }

    struct Release<'a>(&'a OwnedHandle);
    impl Drop for Release<'_> {
        fn drop(&mut self) {
            use std::os::windows::io::AsRawHandle as _;

            // SAFETY: the current thread owns the mutex.
            unsafe { synchapi::ReleaseMutex(self.0.as_raw_handle().cast()) };
        }
    }

    std::io::stdout().flush()?;
    let title = wide(title);
    let set_title = || {
        // SAFETY: `title` is a valid NUL-terminated UTF-16 title.
        if unsafe { wincon::SetConsoleTitleW(title.as_ptr()) } == 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    };
    let Ok(terminal) = std::env::var(MILESTONE_LOCK_ENV) else {
        return set_title();
    };

    let lock_name = wide(&format!("Local\\vt-pty-terminal-test-lock-{terminal}"));
    // SAFETY: `lock_name` is a valid NUL-terminated UTF-16 name.
    let lock =
        check_handle(unsafe { synchapi::CreateMutexW(ptr::null_mut(), 0, lock_name.as_ptr()) })?;
    wait(&lock)?;
    let _release = Release(&lock);

    let ack_name = wide(&milestone_ack_event_name(id));
    // SAFETY: `ack_name` is a valid NUL-terminated UTF-16 name. The event is
    // manual-reset and starts unsignaled.
    let ack =
        check_handle(unsafe { synchapi::CreateEventW(ptr::null_mut(), 1, 0, ack_name.as_ptr()) })?;
    set_title()?;
    wait(&ack)
}

#[cfg(all(feature = "testing", not(windows)))]
fn emit_title(_id: u128, title: &str) -> std::io::Result<()> {
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
        let id = new_milestone_id();
        let title = encode_milestone_title(id, "task-select:lib#:0");
        assert_eq!(
            decode_milestone_title(title.as_bytes()),
            Some(Milestone { id, name: "task-select:lib#:0".to_owned() })
        );
    }

    #[test]
    fn repeated_names_get_unique_ids() {
        assert_ne!(new_milestone_id(), new_milestone_id());
    }

    #[test]
    fn ignores_normal_and_malformed_titles() {
        assert!(decode_milestone_title(b"normal title").is_none());
        assert!(decode_milestone_title(b"vt-pty-terminal-test:not-hex:cmVhZHk").is_none());
        assert!(
            decode_milestone_title(b"vt-pty-terminal-test:00000000000000000000000000000000:*")
                .is_none()
        );
    }
}
