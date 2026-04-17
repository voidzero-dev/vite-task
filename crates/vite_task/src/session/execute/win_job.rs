//! Win32 Job Object utilities for process tree management.
//!
//! On Windows, `TerminateProcess` only kills the direct child process, not its
//! descendants. This module creates a Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`,
//! which automatically terminates all processes in the job when the handle is dropped.

use std::{io, os::windows::io::RawHandle};

use winapi::{
    shared::minwindef::FALSE,
    um::{
        handleapi::CloseHandle,
        jobapi2::{
            AssignProcessToJobObject, CreateJobObjectW, SetInformationJobObject, TerminateJobObject,
        },
        winnt::{HANDLE, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION},
    },
};

/// RAII wrapper around a Win32 Job Object `HANDLE` that closes it on drop.
pub(super) struct OwnedJobHandle(HANDLE);

impl OwnedJobHandle {
    /// Immediately terminate all processes in the job.
    ///
    /// This is needed when pipes to a grandchild process must be closed before
    /// the job handle is dropped (e.g., to unblock pipe reads in `spawn`).
    pub(super) fn terminate(&self) {
        // SAFETY: self.0 is a valid job handle from CreateJobObjectW.
        unsafe { TerminateJobObject(self.0, 1) };
    }
}

impl Drop for OwnedJobHandle {
    fn drop(&mut self) {
        // SAFETY: self.0 is a valid handle obtained from CreateJobObjectW.
        unsafe { CloseHandle(self.0) };
    }
}

/// Create a Job Object with `KILL_ON_JOB_CLOSE` and assign a process to it.
///
/// Returns the job handle wrapped in an RAII guard. When dropped, all processes
/// in the job (the child and its descendants) are terminated.
pub(super) fn assign_to_kill_on_close_job(process_handle: RawHandle) -> io::Result<OwnedJobHandle> {
    // SAFETY: Creating an anonymous job object with no security attributes.
    let job = unsafe { CreateJobObjectW(std::ptr::null_mut(), std::ptr::null()) };
    if job.is_null() {
        return Err(io::Error::last_os_error());
    }
    let job = OwnedJobHandle(job);

    // Configure the job to kill all processes when the handle is closed.
    // SAFETY: JOBOBJECT_EXTENDED_LIMIT_INFORMATION is a plain C struct (no pointers
    // in the zeroed fields). Zeroing then setting LimitFlags is the standard pattern.
    let mut info = unsafe {
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        info
    };

    // SAFETY: info is a valid JOBOBJECT_EXTENDED_LIMIT_INFORMATION, job.0 is a valid handle.
    let ok = unsafe {
        SetInformationJobObject(
            job.0,
            // JobObjectExtendedLimitInformation = 9
            9,
            std::ptr::from_mut(&mut info).cast(),
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>().try_into().unwrap(),
        )
    };
    if ok == FALSE {
        return Err(io::Error::last_os_error());
    }

    // SAFETY: Both handles are valid — job from CreateJobObjectW, process handle
    // from the caller.
    let ok = unsafe { AssignProcessToJobObject(job.0, process_handle as HANDLE) };
    if ok == FALSE {
        return Err(io::Error::last_os_error());
    }

    Ok(job)
}
