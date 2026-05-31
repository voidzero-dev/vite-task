pub mod arg;

use std::{io, os::fd::OwnedFd};

use libc::seccomp_notif;

#[expect(clippy::module_name_repetitions, reason = "clearer as a standalone export")]
pub trait SeccompNotifyHandler {
    fn syscalls() -> &'static [syscalls::Sysno];
    /// Handles a seccomp notification for an intercepted syscall.
    ///
    /// # Errors
    /// Returns an error if the handler fails to process the notification.
    fn handle_notify(&mut self, notify: &seccomp_notif) -> io::Result<()>;
}

/// How the supervisor replies to the kernel for the most recently handled
/// notification.
#[derive(Debug, Default)]
pub enum NotifyResponse {
    /// Let the kernel run the intercepted syscall in the target unchanged.
    #[default]
    Continue,
    /// Install the given file descriptor into the target and complete the
    /// syscall with it as its result. Used by the blocking-callback path so
    /// the supervisor can open a file itself and hand it to the target.
    ReturnFd {
        /// The supervisor-owned descriptor to install into the target.
        fd: OwnedFd,
        /// Whether the installed descriptor should be close-on-exec.
        cloexec: bool,
    },
}

/// Lets a handler override the supervisor's reply for the last notification.
///
/// Kept separate from [`SeccompNotifyHandler`] so the [`impl_handler!`](crate::impl_handler)
/// macro does not need to know about it: handlers that always continue rely on
/// the default implementation.
pub trait HandlerResponse {
    /// Take the response for the notification just processed by
    /// [`SeccompNotifyHandler::handle_notify`]. Defaults to
    /// [`NotifyResponse::Continue`].
    fn take_response(&mut self) -> NotifyResponse {
        NotifyResponse::Continue
    }
}

#[doc(hidden)] // Re-export for use in the macro
pub use syscalls::Sysno;

#[macro_export]
macro_rules! impl_handler {
    ($type:ty: $(
        $(#[$attr:meta])?
        $syscall:ident,
    )* ) => {

    impl $crate::supervisor::handler::SeccompNotifyHandler for $type {
        fn syscalls() -> &'static [$crate::supervisor::handler::Sysno] {
            &[ $(
                $(#[$attr])?
                $crate::supervisor::handler::Sysno::$syscall
            ),* ]
        }
        fn handle_notify(&mut self, notify: &::libc::seccomp_notif) -> ::std::io::Result<()> {
            $crate::supervisor::handler::arg::Caller::with_pid(notify.pid as _, |caller| {
                $(
                    $(#[$attr])?
                    if notify.data.nr == $crate::supervisor::handler::Sysno::$syscall as _ {
                        return self.$syscall(caller, $crate::supervisor::handler::arg::FromNotify::from_notify(notify)?)
                    }
                )*
                Ok(())
            })
        }
    }
    };
}
