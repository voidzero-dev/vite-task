use core::{ffi::c_void, marker::PhantomData, ptr::NonNull};

use windows_sys::Win32::{Security::SECURITY_ATTRIBUTES, System::LibraryLoader::GetModuleHandleW};

use crate::{Result, WideCStr};

/// A raw Windows kernel handle.
pub type RawHandle = *mut c_void;

/// A borrowed Windows kernel handle.
///
/// Its lifetime is tied to the value that keeps the handle open. The sentinel
/// values `NULL` and `-1` are also permitted because some Win32 calls define
/// them as meaningful arguments.
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct BorrowedHandle<'handle> {
    handle: RawHandle,
    lifetime: PhantomData<&'handle OwnedHandle>,
}

/// An owned Windows kernel handle.
///
/// The handle is closed on drop, so sentinel values such as `NULL` and `-1`
/// are never permitted here.
#[repr(transparent)]
pub struct OwnedHandle {
    handle: RawHandle,
}

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

impl BorrowedHandle<'_> {
    /// Borrows a raw handle.
    ///
    /// # Safety
    ///
    /// `handle` must be a valid open handle that remains open for the lifetime
    /// of the returned value, or a sentinel value (`NULL` or `-1`) that every
    /// call receiving the borrow defines as meaningful.
    #[must_use]
    pub const unsafe fn borrow_raw(handle: RawHandle) -> Self {
        Self { handle, lifetime: PhantomData }
    }

    /// Returns the raw handle without transferring ownership.
    #[must_use]
    pub const fn as_raw_handle(&self) -> RawHandle {
        self.handle
    }
}

impl OwnedHandle {
    /// Creates an owned handle after the caller has validated the raw value.
    ///
    /// # Safety
    ///
    /// `handle` must be a valid, uniquely owned handle that may be closed with
    /// `CloseHandle`.
    pub(crate) const unsafe fn from_raw_handle(handle: RawHandle) -> Self {
        Self { handle }
    }

    /// Borrows this handle.
    #[must_use]
    pub const fn as_handle(&self) -> BorrowedHandle<'_> {
        // SAFETY: `self` keeps the same valid handle open for the returned
        // borrow's lifetime.
        unsafe { BorrowedHandle::borrow_raw(self.handle) }
    }
}

// SAFETY: Windows kernel handles are process-scoped and may be moved between
// threads. Moving this value does not access the referenced kernel object.
unsafe impl Send for OwnedHandle {}
// SAFETY: as above; the borrow does not add thread affinity.
unsafe impl Send for BorrowedHandle<'_> {}
// SAFETY: sharing a handle value does not access the kernel object. Each
// operation is responsible for the synchronization required by that object.
unsafe impl Sync for OwnedHandle {}
// SAFETY: as above; sharing the borrowed value does not access the object.
unsafe impl Sync for BorrowedHandle<'_> {}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: this type owns a valid handle and closes it exactly once.
        let _ = unsafe { windows_sys::Win32::Foundation::CloseHandle(self.handle) };
    }
}

/// Converts a Win32 `BOOL` result into a [`Result`].
///
/// As with [`crate::Error::last_os_error`], call this immediately after the
/// Win32 call whose result it receives.
///
/// # Errors
///
/// Returns the calling thread's last Win32 error when `result` is zero.
pub fn bool_result(result: i32) -> Result<()> {
    if result == 0 { Err(crate::Error::last_os_error()) } else { Ok(()) }
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
    NonNull::new(module).ok_or_else(crate::Error::last_os_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_loaded_system_module() {
        let _module = get_module_handle(crate::wide_cstr!("kernel32.dll")).unwrap();
    }
}
