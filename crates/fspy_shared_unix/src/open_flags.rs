use nix::libc::{O_ACCMODE, O_CREAT, O_DIRECTORY, O_NOFOLLOW, O_RDONLY, c_int};

/// Whether an open with `flags` fails without reading the path, so it isn't
/// recorded.
///
/// An `O_DIRECTORY` open for reading, without `O_CREAT`, opens only a
/// folder. For a path that is there and isn't one, it fails with `ENOTDIR`,
/// or with `ELOOP` for a symlink when `O_NOFOLLOW` is set. libc++'s
/// `std::filesystem::remove_all`, which Node's `fs.rmSync` uses on macOS,
/// opens every path it removes that way, and removes the path as a file when
/// the open fails. A folder, or a path that isn't there, is still recorded:
/// a task that lists a folder that way, as Go's `os.ReadDir` does, finds out
/// whether it's there.
///
/// `is_folder` reports whether the path is a folder, following a final
/// symlink when its argument is `true`, or `None` when that can't be told,
/// as when the path isn't there.
pub fn fails_without_reading(flags: c_int, is_folder: impl FnOnce(bool) -> Option<bool>) -> bool {
    flags & O_DIRECTORY != 0
        && flags & O_ACCMODE == O_RDONLY
        && flags & O_CREAT == 0
        && is_folder(flags & O_NOFOLLOW == 0) == Some(false)
}

#[cfg(test)]
mod tests {
    use nix::libc::{O_CLOEXEC, O_RDWR, O_WRONLY};

    use super::*;

    #[test]
    fn folder_only_opens_of_non_folders_fail_without_reading() {
        let not_a_folder = |_| Some(false);
        // libc++'s `remove_all`.
        assert!(fails_without_reading(
            O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC,
            not_a_folder
        ));
        assert!(fails_without_reading(O_RDONLY | O_DIRECTORY, not_a_folder));
        assert!(!fails_without_reading(O_RDONLY, not_a_folder));
        assert!(!fails_without_reading(O_WRONLY | O_DIRECTORY, not_a_folder));
        assert!(!fails_without_reading(O_RDWR | O_DIRECTORY, not_a_folder));
        assert!(!fails_without_reading(O_RDONLY | O_DIRECTORY | O_CREAT, not_a_folder));

        assert!(!fails_without_reading(O_RDONLY | O_DIRECTORY, |_| Some(true)));
        assert!(!fails_without_reading(O_RDONLY | O_DIRECTORY, |_| None));
    }
}
