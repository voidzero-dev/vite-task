use libc::FILE;

use crate::{client::handle_close, libc::c_int, macros::intercept};

intercept!(close(64): unsafe extern "C" fn(c_int) -> c_int);
unsafe extern "C" fn close(fd: c_int) -> c_int {
    // The pre-close callback runs while `fd` is still valid.
    handle_close(fd);
    // SAFETY: forwarding to the original libc close() with the same argument.
    unsafe { close::original()(fd) }
}

intercept!(fclose(64): unsafe extern "C" fn(stream: *mut FILE) -> c_int);
unsafe extern "C" fn fclose(stream: *mut FILE) -> c_int {
    if !stream.is_null() {
        // SAFETY: `stream` is a non-null FILE* provided by the caller, so
        // `fileno` yields its backing descriptor while it is still open.
        handle_close(unsafe { libc::fileno(stream) });
    }
    // SAFETY: forwarding to the original libc fclose() with the same argument.
    unsafe { fclose::original()(stream) }
}
