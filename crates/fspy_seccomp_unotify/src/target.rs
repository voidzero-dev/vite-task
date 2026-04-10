use std::{
    ffi::OsStr,
    os::{
        fd::AsRawFd,
        unix::{ffi::OsStrExt, net::UnixStream},
    },
};

use libc::sock_filter;
#[cfg(not(target_os = "android"))]
use nix::sys::prctl::set_no_new_privs;
use passfd::FdPassingExt;

use crate::{bindings::install_unotify_filter, payload::SeccompPayload};

/// Installs the seccomp user notification filter and sends the notification fd
/// to the supervisor via the IPC socket.
///
/// # Errors
/// Returns an error if setting no-new-privs fails, the filter cannot be installed,
/// or the IPC socket communication fails.
pub fn install_target(payload: &SeccompPayload) -> nix::Result<()> {
    #[cfg(not(target_os = "android"))]
    set_no_new_privs()?;
    #[cfg(target_os = "android")]
    use libc::{PR_SET_NO_NEW_PRIVS, prctl};

    let ret = unsafe { prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
    if ret != 0 {
        return Err(nix::Error::last());
    }

    let sock_filters =
        payload.filter.0.iter().copied().map(sock_filter::from).collect::<Vec<sock_filter>>();
    let notify_fd = install_unotify_filter(&sock_filters)?;
    let ipc_path = OsStr::from_bytes(&payload.ipc_path);
    let ipc_unix_stream = UnixStream::connect(ipc_path)
        .map_err(|err| nix::Error::try_from(err).unwrap_or(nix::Error::UnknownErrno))?;
    ipc_unix_stream
        .send_fd(notify_fd.as_raw_fd())
        .map_err(|err| nix::Error::try_from(err).unwrap_or(nix::Error::UnknownErrno))?;
    Ok(())
}
