use std::{
    ffi::{CStr, c_char},
    io,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, BorrowedHandle},
    },
    path::Path,
    sync::Arc,
};

use fspy_detours_sys::{DetourCopyPayloadToProcess, DetourUpdateProcessWithDll};
use fspy_shared::{
    ipc::{PathAccess, channel::channel},
    windows::{PAYLOAD_ID, Payload},
};
use futures_util::FutureExt;
use materialized_artifact::{Artifact, artifact};
use ntapi::{ntpsapi::NtResumeProcess, ntrtl::RtlNtStatusToDosError};
use tokio_util::sync::CancellationToken;
use winapi::{
    shared::{minwindef::TRUE, ntdef::NT_SUCCESS},
    um::winbase::CREATE_SUSPENDED,
};
use winsafe::co::{CP, WC};

use crate::{
    ChildTermination, TrackedChild, command::Command, error::SpawnError, ipc::ChannelAccesses,
};

const INTERPOSE_CDYLIB: Artifact =
    artifact!("fspy_preload", "CARGO_CDYLIB_FILE_FSPY_PRELOAD_WINDOWS");

pub struct PathAccessIterable {
    ipc_accesses: ChannelAccesses,
}

impl PathAccessIterable {
    pub fn iter(&self) -> impl Iterator<Item = PathAccess<'_>> {
        self.ipc_accesses.iter_path_accesses()
    }
}

// pub struct TracedProcess {
//     pub child: Child,
//     pub path_access_stream: PathAccessIter,
// }

#[derive(Debug, Clone)]
pub struct SpyImpl {
    ansi_dll_path_with_nul: Arc<CStr>,
}

impl SpyImpl {
    pub fn init_in(path: &Path) -> io::Result<Self> {
        let dll_path = INTERPOSE_CDYLIB.materialize().suffix(".dll").at(path)?;

        let wide_dll_path = dll_path.as_os_str().encode_wide().collect::<Vec<u16>>();
        let mut ansi_dll_path =
            winsafe::WideCharToMultiByte(CP::ACP, WC::NoValue, &wide_dll_path, None, None)
                .map_err(|err| io::Error::from_raw_os_error(err.raw().cast_signed()))?;

        ansi_dll_path.push(0);

        // SAFETY: we just pushed a NUL byte, so the slice is NUL-terminated
        let ansi_dll_path_with_nul =
            unsafe { CStr::from_bytes_with_nul_unchecked(ansi_dll_path.as_slice()) };
        Ok(Self { ansi_dll_path_with_nul: ansi_dll_path_with_nul.into() })
    }

    pub(crate) fn spawn(
        &self,
        command: Command,
        cancellation_token: CancellationToken,
    ) -> std::future::Ready<Result<TrackedChild, SpawnError>> {
        std::future::ready(self.spawn_inner(command, cancellation_token))
    }

    fn spawn_inner(
        &self,
        mut command: Command,
        cancellation_token: CancellationToken,
    ) -> Result<TrackedChild, SpawnError> {
        let ansi_dll_path_with_nul = &self.ansi_dll_path_with_nul;
        command.env("FSPY", "1");

        let receiver = channel(crate::ipc::shm_capacity(), allocator_api2::alloc::Global)
            .map_err(SpawnError::ChannelCreation)?;

        let payload = Payload {
            channel_conf: receiver.conf(),
            ansi_dll_path_with_nul: ansi_dll_path_with_nul.to_bytes(),
        };
        let payload_bytes = wincode::serialize(&payload).unwrap();
        let payload_len = payload_bytes.len().try_into().unwrap();

        let mut command = command.into_tokio_command();
        command.creation_flags(CREATE_SUSPENDED);
        let mut child = command.spawn().map_err(SpawnError::OsSpawn)?;

        let preparation = (|| {
            // Duplicate the process handle before the child is moved into the background
            // task so it stays valid after Tokio closes its copy when the process exits.
            // SAFETY: the child owns this handle and is not waited on during this borrow.
            let process = unsafe { BorrowedHandle::borrow_raw(child.raw_handle().unwrap()) };
            let process_handle = process.try_clone_to_owned().map_err(SpawnError::OsSpawn)?;
            let raw_process = process_handle.as_raw_handle().cast::<winapi::ctypes::c_void>();
            let mut dll_paths = ansi_dll_path_with_nul.as_ptr().cast::<c_char>();
            // SAFETY: raw_process is a valid handle to the suspended child process,
            // dll_paths points to a valid null-terminated ANSI string.
            let success = unsafe { DetourUpdateProcessWithDll(raw_process, &raw mut dll_paths, 1) };
            if success != TRUE {
                return Err(SpawnError::Injection(io::Error::last_os_error()));
            }

            // SAFETY: raw_process is valid, PAYLOAD_ID is a static GUID,
            // payload_bytes is a valid buffer with the correct length.
            let success = unsafe {
                DetourCopyPayloadToProcess(
                    raw_process,
                    &PAYLOAD_ID,
                    payload_bytes.as_ptr().cast(),
                    payload_len,
                )
            };
            if success != TRUE {
                return Err(SpawnError::Injection(io::Error::last_os_error()));
            }

            // Resume using the process handle, without the nightly main-thread handle API.
            // SAFETY: raw_process is a valid child process handle with PROCESS_SUSPEND_RESUME access.
            let status = unsafe { NtResumeProcess(raw_process) };
            if !NT_SUCCESS(status) {
                // SAFETY: RtlNtStatusToDosError accepts any NTSTATUS value. Native APIs
                // return their status directly; GetLastError would report a stale error.
                let error = unsafe { RtlNtStatusToDosError(status) };
                return Err(SpawnError::Injection(io::Error::from_raw_os_error(
                    error.cast_signed(),
                )));
            }

            Ok(process_handle)
        })();

        let process_handle = preparation.inspect_err(|_| {
            // Do not leave a suspended process behind if tracking initialization fails.
            let _ = child.start_kill();
        })?;

        Ok(TrackedChild {
            id: child.id().expect("newly spawned child has a process ID"),
            stdin: child.stdin.take(),
            stdout: child.stdout.take(),
            stderr: child.stderr.take(),
            process_handle,
            // Keep polling for the child to exit in the background even if `wait_handle` is not awaited,
            // because we need to stop the supervisor and close the channel as soon as the child exits.
            wait_handle: tokio::spawn(async move {
                let status = tokio::select! {
                    status = child.wait() => status?,
                    () = cancellation_token.cancelled() => {
                        child.start_kill()?;
                        child.wait().await?
                    }
                };
                // Close the ipc channel after the child has exited.
                // We are not interested in path accesses from descendants after the main child has exited.
                let path_accesses = ChannelAccesses::try_from(receiver)
                    .map(|ipc_accesses| PathAccessIterable { ipc_accesses });

                io::Result::Ok(ChildTermination { status, path_accesses })
            })
            .map(|f| f?) // flatten JoinError and io::Result
            .boxed(),
        })
    }
}
