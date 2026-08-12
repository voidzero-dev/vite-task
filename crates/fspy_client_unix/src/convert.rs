use std::ffi::CStr;

use allocator_api2::{alloc::Allocator, vec::Vec};
use bstr::ByteSlice;
use fspy_nostd::{AsRawFd as _, BorrowedFd, CWD};
use fspy_shared::ipc::AccessMode;
use libc::{c_char, c_int};

#[cfg(target_os = "linux")]
fn get_fd_path<A: Allocator>(allocator: A, fd: BorrowedFd<'_>) -> nix::Result<Option<Vec<u8, A>>> {
    if fd.as_raw_fd() == CWD.as_raw_fd() {
        let path = fspy_nostd_alloc::fs::getcwd(allocator)
            .map_err(|errno| nix::errno::Errno::from_raw(errno.raw_os_error()))?
            .into_bytes();
        return Ok(Some(path));
    }
    let mut path = [0; PROC_FD_PATH_CAPACITY];
    let path = proc_fd_path(fd, &mut path);
    match fspy_nostd_alloc::fs::readlinkat(allocator, CWD, path) {
        Ok(path) => Ok(Some(path)),
        Err(fspy_nostd::Errno::BADF | fspy_nostd::Errno::NOENT) => Ok(None),
        Err(errno) => Err(nix::errno::Errno::from_raw(errno.raw_os_error())),
    }
}

#[cfg(target_os = "linux")]
const PROC_FD_PATH_CAPACITY: usize =
    b"/proc/self/fd/".len() + <c_int as itoa::Integer>::MAX_STR_LEN + 1;

#[cfg(target_os = "linux")]
fn proc_fd_path<'buf>(
    fd: BorrowedFd<'_>,
    buf: &'buf mut [u8; PROC_FD_PATH_CAPACITY],
) -> fspy_nostd::CStr<'buf, fspy_nostd::Thin> {
    const PREFIX: &[u8] = b"/proc/self/fd/";

    let mut formatted = itoa::Buffer::new();
    let fd = formatted.format(fd.as_raw_fd()).as_bytes();

    buf[..PREFIX.len()].copy_from_slice(PREFIX);
    let end = PREFIX.len() + fd.len();
    buf[PREFIX.len()..end].copy_from_slice(fd);
    buf[end] = 0;

    // SAFETY: the initialized prefix ends in NUL and lives as long as `buf`.
    unsafe { fspy_nostd::CStr::from_ptr(buf.as_ptr().cast()) }
}

#[cfg(target_os = "macos")]
fn get_fd_path<A: Allocator>(allocator: A, fd: BorrowedFd<'_>) -> nix::Result<Option<Vec<u8, A>>> {
    if fd.as_raw_fd() == CWD.as_raw_fd() {
        let path = fspy_nostd_alloc::fs::getcwd(allocator)
            .map_err(|errno| nix::errno::Errno::from_raw(errno.raw_os_error()))?
            .into_bytes();
        return Ok(Some(path));
    }

    match fspy_nostd_alloc::fs::fcntl_getpath(allocator, fd) {
        Ok(path) => {
            // `F_GETPATH` does not return a length. Count at this caller before
            // converting its allocation into the returned path.
            Ok(Some(path.count().into_bytes()))
        }
        Err(fspy_nostd::Errno::BADF | fspy_nostd::Errno::NOENT) => Ok(None),
        Err(errno) => Err(nix::errno::Errno::from_raw(errno.raw_os_error())),
    }
}

pub trait ToAbsolutePath {
    /// Resolves this argument to an absolute path allocated in `allocator`,
    /// or borrowed from the argument itself when it is already absolute.
    ///
    /// The result is a C string so that callers forwarding it to an exec —
    /// which needs a terminator — cannot be handed unterminated bytes;
    /// [`as_bytes`] gives the path without the NUL.
    ///
    /// [`as_bytes`]: fspy_nostd::CStr::as_bytes
    ///
    /// # Errors
    ///
    /// Returns the error reported while resolving a descriptor or the current
    /// working directory.
    fn to_absolute_path<'a, A: Allocator>(
        self,
        allocator: &'a A,
    ) -> nix::Result<Option<fspy_nostd::CStr<'a, fspy_nostd::Fat>>>
    where
        Self: 'a;
}

impl ToAbsolutePath for BorrowedFd<'_> {
    fn to_absolute_path<'a, A: Allocator>(
        self,
        allocator: &'a A,
    ) -> nix::Result<Option<fspy_nostd::CStr<'a, fspy_nostd::Fat>>>
    where
        Self: 'a,
    {
        let Some(mut path) = get_fd_path(allocator, self)? else {
            return Ok(None);
        };
        path.push(0);
        // SAFETY: a resolved descriptor path carries no interior NUL, and
        // exactly one was appended above. The storage stays in `allocator`
        // until it is dropped, which for a per-call arena ends the call.
        Ok(Some(unsafe { fspy_nostd::CStr::from_bytes_with_nul_unchecked(path.leak()) }))
    }
}

pub struct PathAt<'fd, 'path>(pub BorrowedFd<'fd>, pub fspy_nostd::CStr<'path, fspy_nostd::Thin>);

impl PathAt<'_, '_> {
    /// Borrows raw directory-descriptor and pathname arguments.
    ///
    /// # Safety
    ///
    /// `fd` must remain valid while the returned value is used, and `path`
    /// must point to a valid NUL-terminated string.
    #[must_use]
    pub const unsafe fn borrow_raw(fd: c_int, path: *const c_char) -> Self {
        // SAFETY: both invariants are upheld by the caller.
        Self(unsafe { BorrowedFd::borrow_raw(fd) }, unsafe { fspy_nostd::CStr::from_ptr(path) })
    }
}

impl ToAbsolutePath for PathAt<'_, '_> {
    fn to_absolute_path<'a, A: Allocator>(
        self,
        allocator: &'a A,
    ) -> nix::Result<Option<fspy_nostd::CStr<'a, fspy_nostd::Fat>>>
    where
        Self: 'a,
    {
        let counted = self.1.count();
        let pathname = counted.as_bytes();

        if pathname.starts_with(b"/") {
            // Already absolute, and already NUL-terminated by the caller.
            Ok(Some(counted))
        } else {
            let Some(mut base) = get_fd_path(allocator, self.0)? else {
                return Ok(None);
            };
            if !pathname.is_empty() {
                if !base.ends_with(b"/") {
                    base.push(b'/');
                }
                base.extend_from_slice(pathname);
            }
            base.push(0);
            // SAFETY: neither the directory nor the pathname carries an
            // interior NUL — both come from C strings or the kernel — and
            // exactly one was appended above. The storage stays in
            // `allocator` until it is dropped.
            Ok(Some(unsafe { fspy_nostd::CStr::from_bytes_with_nul_unchecked(base.leak()) }))
        }
    }
}

impl ToAbsolutePath for fspy_nostd::CStr<'_, fspy_nostd::Thin> {
    fn to_absolute_path<'a, A: Allocator>(
        self,
        allocator: &'a A,
    ) -> nix::Result<Option<fspy_nostd::CStr<'a, fspy_nostd::Fat>>>
    where
        Self: 'a,
    {
        PathAt(CWD, self).to_absolute_path(allocator)
    }
}

pub trait ToAccessMode {
    /// Converts the intercepted operation's mode into an access mode.
    ///
    /// # Safety
    ///
    /// Implementations backed by raw process pointers require those pointers
    /// to remain valid for the conversion.
    unsafe fn to_access_mode(self) -> AccessMode;
}

impl ToAccessMode for AccessMode {
    unsafe fn to_access_mode(self) -> AccessMode {
        self
    }
}

pub struct OpenFlags(pub c_int);
impl ToAccessMode for OpenFlags {
    unsafe fn to_access_mode(self) -> AccessMode {
        match self.0 & libc::O_ACCMODE {
            libc::O_RDWR => AccessMode::READ | AccessMode::WRITE,
            libc::O_WRONLY => AccessMode::WRITE,
            _ => AccessMode::READ,
        }
    }
}

pub struct ModeStr(pub *const c_char);
impl ToAccessMode for ModeStr {
    unsafe fn to_access_mode(self) -> AccessMode {
        // SAFETY: self.0 is a non-null pointer to a valid null-terminated C
        // string, as guaranteed by the libc calling convention.
        let mode_str = unsafe { CStr::from_ptr(self.0) }.to_bytes().as_bstr();
        let has_read = mode_str.contains(&b'r');
        let has_write = mode_str.contains(&b'w') || mode_str.contains(&b'a');
        match (has_read, has_write) {
            (false, true) => AccessMode::WRITE,
            (true, true) => AccessMode::READ | AccessMode::WRITE,
            _ => AccessMode::READ,
        }
    }
}
