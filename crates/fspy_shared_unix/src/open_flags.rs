use fspy_shared::ipc::AccessMode;
use nix::libc::{O_ACCMODE, O_DIRECTORY, O_RDWR, O_WRONLY, c_int};

/// The access an open with `flags` is recorded as, or `None` if it isn't
/// recorded.
///
/// An `O_DIRECTORY` open reads nothing: it opens only a folder, and reading
/// the folder's entries is recorded on its own. libc++'s
/// `std::filesystem::remove_all`, which Node's `fs.rmSync` uses on macOS,
/// opens every path it removes that way and removes the path as a file when
/// the open fails, so recording the open would make every file it removes an
/// input. On Linux, `O_TMPFILE` includes `O_DIRECTORY`; such an open makes a
/// file without a name, and giving it one with `linkat` is recorded as a
/// write.
#[must_use]
pub const fn open_access_mode(flags: c_int) -> Option<AccessMode> {
    if flags & O_DIRECTORY != 0 {
        return None;
    }
    Some(match flags & O_ACCMODE {
        O_RDWR => AccessMode::ReadWrite,
        O_WRONLY => AccessMode::Write,
        _ => AccessMode::Read,
    })
}
