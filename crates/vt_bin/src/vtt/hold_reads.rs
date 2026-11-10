/// hold-reads `<file>` `<dir>`
///
/// Holds other processes' reads of `<file>` until another process releases
/// them, for testing that a blocked read doesn't stop other work:
///
/// 1. Make opening `<file>` wait, then join the barrier `<dir>` `held` 2,
///    so that its other participant knows a read of `<file>` would wait.
/// 2. Wait until another process opens `<file>`.
/// 3. Join the barrier `<dir>` `released` 2, then let the waiting read finish
///    with the file's content and exit.
///
/// On Unix, `<file>` is replaced by a FIFO, whose `open` waits for a writer.
/// The file is put back as soon as a reader opens the FIFO, so later opens
/// don't wait. On Windows, an exclusive oplock is taken on `<file>`. Another
/// process's open breaks it and waits until the oplock's handle is closed.
pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let [file, dir] = args else {
        return Err("Usage: vtt hold-reads <file> <dir>".into());
    };
    hold(std::path::Path::new(file), std::path::Path::new(dir))
}

#[cfg(unix)]
fn hold(file: &std::path::Path, dir: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    use std::{io::Write as _, os::unix::ffi::OsStrExt as _};

    let content = std::fs::read(file)?;
    let mut held = file.as_os_str().to_owned();
    held.push(".held");
    std::fs::write(&held, &content)?;
    std::fs::remove_file(file)?;
    let fifo_path = std::ffi::CString::new(file.as_os_str().as_bytes())?;
    // SAFETY: `fifo_path` is a valid NUL-terminated path.
    if unsafe { libc::mkfifo(fifo_path.as_ptr(), 0o644) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    crate::barrier::wait(dir, "held", 2)?;

    // Opening a FIFO to write waits until a reader opens it.
    let mut fifo = std::fs::OpenOptions::new().write(true).open(file)?;
    // The reader keeps the FIFO open, and later opens get the file.
    std::fs::rename(&held, file)?;
    crate::barrier::wait(dir, "released", 2)?;
    // The reader reads the content, then the end of the file once `fifo` is
    // closed.
    fifo.write_all(&content)?;
    Ok(())
}

#[cfg(windows)]
fn hold(file: &std::path::Path, dir: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    use std::os::windows::{fs::OpenOptionsExt as _, io::AsRawHandle as _};

    use windows_sys::Win32::{
        Foundation::ERROR_IO_PENDING,
        Storage::FileSystem::FILE_FLAG_OVERLAPPED,
        System::{
            IO::{DeviceIoControl, GetOverlappedResult, OVERLAPPED},
            Ioctl::FSCTL_REQUEST_OPLOCK_LEVEL_1,
        },
    };

    // The oplock request stays pending while the oplock is held, and is
    // completed when it breaks. Leaked so that it outlives the request on
    // every path.
    // SAFETY: `OVERLAPPED` is a plain C struct, valid when zeroed.
    let overlapped: *mut OVERLAPPED = Box::into_raw(Box::new(unsafe { std::mem::zeroed() }));
    // The default share mode lets the other process open the file, which
    // breaks the oplock.
    let held =
        std::fs::OpenOptions::new().read(true).custom_flags(FILE_FLAG_OVERLAPPED).open(file)?;
    let handle = held.as_raw_handle();
    // SAFETY: `handle` is an open file handle for overlapped I/O, and
    // `overlapped` lives until the process exits. A level 1 oplock request
    // takes no buffers.
    let requested = unsafe {
        DeviceIoControl(
            handle,
            FSCTL_REQUEST_OPLOCK_LEVEL_1,
            std::ptr::null(),
            0,
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            overlapped,
        )
    };
    let error = std::io::Error::last_os_error();
    if requested != 0 {
        return Err("the oplock request completed without holding the oplock".into());
    }
    if error.raw_os_error() != i32::try_from(ERROR_IO_PENDING).ok() {
        return Err(std::format!("failed to take an oplock: {error}").into());
    }
    crate::barrier::wait(dir, "held", 2)?;

    let mut transferred = 0;
    // SAFETY: `overlapped` is the pending request on `handle`. With no event
    // in it, the wait is on `handle`, which has no other I/O.
    if unsafe { GetOverlappedResult(handle, overlapped, &raw mut transferred, 1) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    crate::barrier::wait(dir, "released", 2)?;
    // Closing the handle acknowledges the break, and the waiting open
    // proceeds.
    drop(held);
    Ok(())
}
