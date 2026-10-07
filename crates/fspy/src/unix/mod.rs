#[cfg(target_os = "linux")]
mod syscall_handler;

#[cfg(target_os = "macos")]
mod macos_artifacts;

#[cfg(target_os = "linux")]
use std::sync::Arc;
use std::{io, path::Path};

#[cfg(target_os = "linux")]
use fspy_seccomp_unotify::supervisor::supervise;
#[cfg(not(target_env = "musl"))]
use fspy_shared::ipc::IpcStr;
use fspy_shared::ipc::{PathAccess, channel::channel};
use fspy_shared_unix::{
    exec::ExecResolveConfig,
    payload::{Payload, encode_payload},
    spawn::handle_exec,
};
use futures_util::FutureExt;
#[cfg(target_os = "linux")]
use syscall_handler::SyscallHandler;
use tokio::task::spawn_blocking;
use tokio_util::sync::CancellationToken;

use crate::{
    ChildTermination, Command, TrackedChild, arena::PathAccessArena, error::SpawnError,
    ipc::ChannelAccesses,
};

#[derive(Debug)]
#[cfg_attr(
    target_os = "macos",
    expect(clippy::struct_field_names, reason = "each field names a distinct injected path")
)]
pub struct SpyImpl {
    #[cfg(target_os = "macos")]
    bash_path: Box<IpcStr>,
    #[cfg(target_os = "macos")]
    coreutils_path: Box<IpcStr>,

    #[cfg(not(target_env = "musl"))]
    preload_path: Box<IpcStr>,
}

impl SpyImpl {
    /// Initialize the fs access spy by writing the preload library on disk.
    ///
    /// On musl targets, we don't build a preload library —
    /// only seccomp-based tracking is used.
    pub fn init_in(#[cfg_attr(target_env = "musl", allow(unused))] dir: &Path) -> io::Result<Self> {
        #[cfg(not(target_env = "musl"))]
        let preload_path = {
            use materialized_artifact::{Artifact, artifact};

            const PRELOAD_CDYLIB: Artifact =
                artifact!("fspy_preload", "CARGO_CDYLIB_FILE_FSPY_PRELOAD_UNIX");

            let preload_cdylib_path = PRELOAD_CDYLIB.materialize().suffix(".dylib").at(dir)?;
            preload_cdylib_path.as_path().into()
        };

        Ok(Self {
            #[cfg(not(target_env = "musl"))]
            preload_path,
            #[cfg(target_os = "macos")]
            bash_path: macos_artifacts::OILS_BINARY
                .materialize()
                .executable()
                .at(dir)?
                .as_path()
                .into(),
            #[cfg(target_os = "macos")]
            coreutils_path: macos_artifacts::COREUTILS_BINARY
                .materialize()
                .executable()
                .at(dir)?
                .as_path()
                .into(),
        })
    }

    pub(crate) async fn spawn(
        &self,
        mut command: Command,
        cancellation_token: CancellationToken,
    ) -> Result<TrackedChild, SpawnError> {
        let ipc_receiver = channel(crate::ipc::shm_capacity(), allocator_api2::alloc::Global)
            .map_err(SpawnError::ChannelCreation)?;

        // The supervisor records the accesses it intercepts into the same
        // channel as the preload library.
        #[cfg(target_os = "linux")]
        let supervisor = {
            let ipc_sender = Arc::new(ipc_receiver.sender().map_err(SpawnError::ChannelCreation)?);
            supervise(move || SyscallHandler::new(Arc::clone(&ipc_sender)))
                .map_err(SpawnError::Supervisor)?
        };

        let payload = Payload {
            #[cfg(not(target_env = "musl"))]
            ipc_channel_conf: ipc_receiver.conf(),
            #[cfg(target_env = "musl")]
            ipc_channel_conf: core::marker::PhantomData,

            #[cfg(not(target_env = "musl"))]
            preload_path: &self.preload_path,

            #[cfg(target_os = "macos")]
            artifacts: fspy_shared_unix::payload::Artifacts {
                bash_path: &self.bash_path,
                coreutils_path: &self.coreutils_path,
            },

            #[cfg(target_os = "linux")]
            seccomp_payload: supervisor.payload().clone(),
        };

        // Spawn-scoped storage for the encoded payload, freed when this
        // spawn returns.
        let payload_bump = bumpalo::Bump::new();
        let encoded_payload = encode_payload(payload, &payload_bump);

        let mut exec = command.get_exec();
        let mut exec_resolve_accesses = PathAccessArena::default();
        let pre_exec = handle_exec(
            &mut exec,
            ExecResolveConfig::search_path_enabled(None),
            &encoded_payload,
            |mode, path| {
                exec_resolve_accesses.add(PathAccess { mode, path: path.into() });
            },
        )
        .map_err(|err| SpawnError::Injection(err.into()))?;
        command.set_exec(exec);
        command.env("FSPY", "1");

        let mut tokio_command = command.into_tokio_command();

        // SAFETY: the pre_exec closure only calls pre_exec.run() which is safe to call in a fork context
        unsafe {
            tokio_command.pre_exec(move || {
                if let Some(pre_exec) = pre_exec.as_ref() {
                    pre_exec.run()?;
                }
                Ok(())
            });
        }

        // tokio_command.spawn blocks while executing the `pre_exec` closure.
        // Run it inside spawn_blocking to avoid blocking the tokio runtime, especially the supervisor loop,
        // which needs to accept incoming connections while `pre_exec` is connecting to it.
        let mut child = spawn_blocking(move || tokio_command.spawn())
            .await
            .map_err(|err| SpawnError::OsSpawn(err.into()))?
            .map_err(SpawnError::OsSpawn)?;

        Ok(TrackedChild {
            stdin: child.stdin.take(),
            stdout: child.stdout.take(),
            stderr: child.stderr.take(),
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

                // Stop the supervisor before closing the channel, so the
                // accesses it intercepted are all recorded in the channel.
                #[cfg(target_os = "linux")]
                supervisor.stop().await?;

                // Close the ipc channel after the child has exited.
                // We are not interested in path accesses from descendants after the main child has exited.
                let path_accesses = ChannelAccesses::try_from(ipc_receiver)
                    .map(|ipc_accesses| PathAccessIterable { exec_resolve_accesses, ipc_accesses });

                io::Result::Ok(ChildTermination { status, path_accesses })
            })
            .map(|f| f?) // flatten JoinError and io::Result
            .boxed(),
        })
    }
}

pub struct PathAccessIterable {
    exec_resolve_accesses: PathAccessArena,
    ipc_accesses: ChannelAccesses,
}

impl PathAccessIterable {
    /// Iterates over the path accesses in the order they were made.
    ///
    /// Accesses made at the same time by different threads or processes
    /// appear in an unspecified order relative to each other.
    pub fn iter(&self) -> impl Iterator<Item = PathAccess<'_>> {
        // Resolving the program happens before the child is spawned.
        let accesses_in_arena = self.exec_resolve_accesses.borrow_accesses().iter().copied();
        let accesses_in_shm = self.ipc_accesses.iter_path_accesses();
        accesses_in_arena.chain(accesses_in_shm)
    }
}
