use std::sync::OnceLock;

static IS_NESTED: OnceLock<bool> = OnceLock::new();

/// Returns `true` if the current process is already running inside an active
/// outer `fspy` tracker.
///
/// Probes a hardcoded random-GUID sentinel path located beneath the OS null
/// device (`/dev/null/fspy_probe_<guid>` on Unix, `\??\NUL\fspy_probe_<guid>`
/// on Windows). Because the null device is not a directory, the OS kernel
/// always rejects this lookup with an error; only an active outer `fspy`
/// preload intercepts the exact sentinel path and short-circuits it to success
/// without recording it as a file access.
///
/// The result is cached for the lifetime of the process.
pub fn is_nested() -> bool {
    *IS_NESTED.get_or_init(probe_nested)
}

#[cfg(target_os = "linux")]
fn probe_nested() -> bool {
    use fspy_shared::probe::NESTED_PROBE_PATH_UNIX;
    use syscalls::Sysno;

    // SAFETY: `NESTED_PROBE_PATH_UNIX` is a valid static NUL-terminated C string.
    unsafe {
        syscalls::syscall!(
            Sysno::faccessat,
            libc::AT_FDCWD,
            NESTED_PROBE_PATH_UNIX.as_ptr(),
            libc::F_OK
        ) == Ok(0)
    }
}

#[cfg(target_os = "macos")]
fn probe_nested() -> bool {
    use fspy_shared::probe::NESTED_PROBE_PATH_UNIX;

    // SAFETY: `NESTED_PROBE_PATH_UNIX` is a valid static NUL-terminated C string.
    unsafe { libc::faccessat(libc::AT_FDCWD, NESTED_PROBE_PATH_UNIX.as_ptr(), libc::F_OK, 0) == 0 }
}

#[cfg(target_os = "windows")]
fn probe_nested() -> bool {
    use std::{mem::MaybeUninit, ptr};

    use fspy_shared::probe::{
        NESTED_PROBE_PATH_WINDOWS_BYTE_LEN, NESTED_PROBE_PATH_WINDOWS_UTF16_NUL,
    };
    use ntapi::ntioapi::{FILE_BASIC_INFORMATION, NtQueryAttributesFile};
    use winapi::shared::{
        ntdef::{InitializeObjectAttributes, OBJECT_ATTRIBUTES, UNICODE_STRING},
        ntstatus::STATUS_SUCCESS,
    };

    let mut name = UNICODE_STRING {
        Length: NESTED_PROBE_PATH_WINDOWS_BYTE_LEN,
        MaximumLength: NESTED_PROBE_PATH_WINDOWS_BYTE_LEN + 2,
        Buffer: NESTED_PROBE_PATH_WINDOWS_UTF16_NUL.as_ptr().cast_mut(),
    };
    let mut oa = MaybeUninit::<OBJECT_ATTRIBUTES>::uninit();
    // SAFETY: `InitializeObjectAttributes` initializes all fields of `oa` with
    // our stack-local `UNICODE_STRING` and null root/security descriptor.
    unsafe {
        InitializeObjectAttributes(
            oa.as_mut_ptr(),
            &raw mut name,
            0,
            ptr::null_mut(),
            ptr::null_mut(),
        );
    }
    // SAFETY: `InitializeObjectAttributes` fully initialized `oa`.
    let mut oa = unsafe { oa.assume_init() };
    let mut info = MaybeUninit::<FILE_BASIC_INFORMATION>::uninit();
    // SAFETY: `oa` and `info` are valid pointers for `NtQueryAttributesFile`.
    let status = unsafe { NtQueryAttributesFile(&raw mut oa, info.as_mut_ptr()) };
    status == STATUS_SUCCESS
}
