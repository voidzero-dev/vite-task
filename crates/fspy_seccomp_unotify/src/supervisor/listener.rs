use std::{
    io,
    os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd},
};

use libc::{seccomp_notif, seccomp_notif_resp};
use tokio::io::unix::AsyncFd;
use tracing::trace;

use crate::bindings::{
    alloc::{Alloced, alloc_seccomp_notif},
    notif_recv,
};

pub struct NotifyListener {
    async_fd: AsyncFd<OwnedFd>,
    notif_buf: Alloced<libc::seccomp_notif>,
}

impl TryFrom<OwnedFd> for NotifyListener {
    type Error = io::Error;

    fn try_from(value: OwnedFd) -> Result<Self, Self::Error> {
        Ok(Self { async_fd: AsyncFd::new(value)?, notif_buf: alloc_seccomp_notif() })
    }
}
impl AsFd for NotifyListener {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.async_fd.as_fd()
    }
}

const SECCOMP_IOCTL_NOTIF_SEND: libc::Ioctl = 3_222_806_785u64 as libc::Ioctl;

// `SECCOMP_IOCTL_NOTIF_ADDFD` = `_IOW('!', 3, struct seccomp_notif_addfd)`.
// `struct seccomp_notif_addfd` is 24 bytes; the ioctl number is not exposed by
// the `libc` crate, so it is hand-encoded the same way as the constant above.
const SECCOMP_IOCTL_NOTIF_ADDFD: libc::Ioctl = 1_075_323_139u64 as libc::Ioctl;
// Install the fd and complete the notification atomically (kernel 5.14+).
const SECCOMP_ADDFD_FLAG_SEND: u32 = 1 << 1;

/// Mirror of the kernel `struct seccomp_notif_addfd` (not in the `libc` crate).
#[repr(C)]
struct SeccompNotifAddfd {
    id: u64,
    flags: u32,
    srcfd: u32,
    newfd: u32,
    newfd_flags: u32,
}

impl NotifyListener {
    /// Sends a `SECCOMP_USER_NOTIF_FLAG_CONTINUE` response for the given request ID.
    ///
    /// # Errors
    /// Returns an error if the ioctl call fails, except for `ENOENT` which is
    /// silently ignored (indicates the target process's syscall was interrupted).
    pub fn send_continue(
        &self,
        req_id: u64,
        buf: &mut Alloced<seccomp_notif_resp>,
    ) -> io::Result<()> {
        let resp = buf.zeroed();
        resp.id = req_id;
        #[expect(clippy::cast_possible_truncation, reason = "flag constant fits in u32")]
        {
            resp.flags = libc::SECCOMP_USER_NOTIF_FLAG_CONTINUE as u32;
        }

        // SAFETY: `resp` is a valid mutable pointer to a zeroed and populated
        // `seccomp_notif_resp` buffer, and the fd is a valid seccomp notify fd
        let ret = unsafe {
            libc::ioctl(self.async_fd.as_raw_fd(), SECCOMP_IOCTL_NOTIF_SEND, &raw mut *resp)
        };
        if ret < 0 {
            let err = nix::Error::last();
            // ignore error if target process's syscall was interrupted
            if err == nix::Error::ENOENT {
                return Ok(());
            }
            return Err(err.into());
        }
        Ok(())
    }

    /// Installs `src_fd` into the target process and completes the
    /// notification, so the intercepted syscall returns the freshly installed
    /// descriptor instead of being re-run by the kernel.
    ///
    /// # Errors
    /// Returns an error if the ioctl call fails, except for `ENOENT` which is
    /// silently ignored (indicates the target process's syscall was interrupted).
    pub fn send_addfd_response(
        &self,
        req_id: u64,
        src_fd: BorrowedFd<'_>,
        cloexec: bool,
    ) -> io::Result<()> {
        let addfd = SeccompNotifAddfd {
            id: req_id,
            flags: SECCOMP_ADDFD_FLAG_SEND,
            srcfd: src_fd.as_raw_fd().cast_unsigned(),
            newfd: 0,
            newfd_flags: if cloexec { libc::O_CLOEXEC.cast_unsigned() } else { 0 },
        };

        // SAFETY: `addfd` is a valid, fully-initialized `seccomp_notif_addfd`
        // buffer, and the fd is a valid seccomp notify fd.
        let ret = unsafe {
            libc::ioctl(self.async_fd.as_raw_fd(), SECCOMP_IOCTL_NOTIF_ADDFD, &raw const addfd)
        };
        if ret < 0 {
            let err = nix::Error::last();
            // ignore error if target process's syscall was interrupted
            if err == nix::Error::ENOENT {
                return Ok(());
            }
            return Err(err.into());
        }
        Ok(())
    }

    /// Waits for and returns the next seccomp notification, or `None` if the fd is closed.
    ///
    /// # Errors
    /// Returns an error if waiting on or reading from the notification fd fails.
    pub async fn next(&mut self) -> io::Result<Option<&seccomp_notif>> {
        loop {
            let mut ready_guard = self.async_fd.readable().await?;
            let ready = ready_guard.ready();
            trace!("notify fd readable: {:?}", ready);
            if ready.is_read_closed() || ready.is_write_closed() {
                return Ok(None);
            }

            if !ready.is_readable() {
                continue;
            }
            // TODO: check why this call solves the issue that `is_read_closed || is_write_closed` is never true.
            ready_guard.clear_ready();

            match notif_recv(ready_guard.get_inner().as_fd(), &mut self.notif_buf) {
                Ok(()) => return Ok(Some(&self.notif_buf)),
                Err(nix::Error::EINTR | nix::Error::EWOULDBLOCK | nix::Error::ENOENT) => {}
                Err(other_error) => return Err(other_error.into()),
            }
        }
    }
}
