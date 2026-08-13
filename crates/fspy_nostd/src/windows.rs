use core::{ffi::c_void, ptr::NonNull};

use windows_sys::Win32::{
    Foundation::GetLastError,
    Security::SECURITY_ATTRIBUTES,
    System::{IO::OVERLAPPED, LibraryLoader::GetModuleHandleW},
};

use crate::{Result, WideCStr};

/// An owned Windows kernel handle.
pub struct OwnedHandle(NonNull<c_void>);

/// Opaque security attributes accepted by Win32 creation functions.
///
/// This type has no public constructor. Non-default security attributes will
/// remain unavailable to safe callers until their embedded security
/// descriptor can be represented safely.
#[repr(transparent)]
pub struct SecurityAttributes(SECURITY_ATTRIBUTES);

impl SecurityAttributes {
    pub(crate) const fn as_raw(&self) -> *const SECURITY_ATTRIBUTES {
        &raw const self.0
    }
}

/// Opaque state for overlapped Win32 I/O.
///
/// This type has no public constructor because an asynchronous operation must
/// tie its lifetime to the state and every borrowed buffer.
#[repr(transparent)]
pub struct Overlapped(OVERLAPPED);

impl Overlapped {
    pub(crate) const fn as_raw_mut(&mut self) -> *mut OVERLAPPED {
        &raw mut self.0
    }
}

impl OwnedHandle {
    /// Creates an owned handle after the caller has validated the raw value.
    ///
    /// # Safety
    ///
    /// `handle` must be a valid, non-null, uniquely owned handle that may be
    /// closed with `CloseHandle`.
    pub(crate) const unsafe fn from_raw(handle: *mut c_void) -> Self {
        // SAFETY: the caller guarantees that the handle is non-null.
        Self(unsafe { NonNull::new_unchecked(handle) })
    }

    /// Returns the raw Windows handle without transferring ownership.
    pub(crate) const fn as_raw(&self) -> *mut c_void {
        self.0.as_ptr()
    }
}

// SAFETY: Windows kernel handles are not thread-affine. The operations exposed
// by this crate provide their own synchronization or do not mutate the handle.
unsafe impl Send for OwnedHandle {}
// SAFETY: as above; sharing the value does not itself access the referenced
// kernel object.
unsafe impl Sync for OwnedHandle {}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: this type owns a valid handle and closes it exactly once.
        let _ = unsafe { windows_sys::Win32::Foundation::CloseHandle(self.as_raw()) };
    }
}

pub fn last_error() -> crate::Error {
    // SAFETY: `GetLastError` reads thread-local error state.
    crate::Error::from_raw_os_error(unsafe { GetLastError() })
}

pub fn bool_result(result: i32) -> Result<()> {
    if result == 0 { Err(last_error()) } else { Ok(()) }
}

/// Returns a handle to the loaded module named by `name`.
///
/// This does not load the module or increment its loader reference count. The
/// returned handle must not be passed to `FreeLibrary`.
///
/// # Errors
///
/// Returns the Windows error reported when no matching loaded module exists.
#[expect(clippy::needless_pass_by_value, reason = "CStr is a borrowed value type")]
pub fn get_module_handle<R>(name: WideCStr<'_, R>) -> Result<NonNull<c_void>> {
    // SAFETY: `name` remains a valid NUL-terminated wide string for the call.
    let module = unsafe { GetModuleHandleW(name.as_ptr()) };
    NonNull::new(module).ok_or_else(|| {
        // SAFETY: `GetModuleHandleW` just failed on this thread.
        crate::Error::from_raw_os_error(unsafe { GetLastError() })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_loaded_system_module() {
        let _module = get_module_handle(crate::wide_cstr!("kernel32.dll")).unwrap();
    }
}
