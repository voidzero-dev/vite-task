use core::{ffi::c_void, ptr::NonNull};

use windows_sys::Win32::{Foundation::GetLastError, System::LibraryLoader::GetModuleHandleW};

use crate::{Result, WideCStr};

/// A non-null handle to a module loaded in the current process.
///
/// This handle does not own a loader reference and must not be passed to
/// `FreeLibrary`.
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct ModuleHandle(NonNull<c_void>);

impl ModuleHandle {
    /// Returns the raw Windows module handle.
    #[must_use]
    pub const fn as_ptr(self) -> *mut c_void {
        self.0.as_ptr()
    }
}

/// Returns a handle to the loaded module named by `name`.
///
/// This does not load the module or increment its loader reference count.
///
/// # Errors
///
/// Returns the Windows error reported when no matching loaded module exists.
#[expect(clippy::needless_pass_by_value, reason = "CStr is a borrowed value type")]
pub fn get_module_name<R>(name: WideCStr<'_, R>) -> Result<ModuleHandle> {
    // SAFETY: `name` remains a valid NUL-terminated wide string for the call.
    let module = unsafe { GetModuleHandleW(name.as_ptr()) };
    NonNull::new(module).map(ModuleHandle).ok_or_else(|| {
        // SAFETY: `GetModuleHandleW` just failed on this thread.
        crate::Error::from_raw_os_error(unsafe { GetLastError() })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_loaded_system_module() {
        let module = get_module_name(crate::wide_cstr!("kernel32.dll")).unwrap();
        assert!(!module.as_ptr().is_null());
    }
}
