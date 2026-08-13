use core::{ffi::c_void, ptr::NonNull};

use windows_sys::Win32::{Foundation::GetLastError, System::LibraryLoader::GetModuleHandleW};

use crate::{Result, WideCStr};

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
