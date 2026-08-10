//! Drains in-flight sends before a voluntary exit.
//!
//! A process may call `exit` while another of its threads is between claiming
//! a frame and finishing it. Dying there abandons the frame's gate guard, and
//! the runner then treats the whole run's tracking as incomplete. The drain
//! lasts microseconds; signals and crashes still skip it, and the runner
//! handles those by not caching the run.

use libc::c_int;

use crate::{client::drain_in_flight_sends, macros::intercept};

intercept!(exit: unsafe extern "C" fn(status: c_int) -> !);
unsafe extern "C" fn exit(status: c_int) -> ! {
    drain_in_flight_sends();
    // SAFETY: forwarding to the real libc exit with the caller's status
    unsafe { exit::original()(status) }
}

intercept!(_exit: unsafe extern "C" fn(status: c_int) -> !);
unsafe extern "C" fn _exit(status: c_int) -> ! {
    drain_in_flight_sends();
    // SAFETY: forwarding to the real libc _exit with the caller's status
    unsafe { _exit::original()(status) }
}
