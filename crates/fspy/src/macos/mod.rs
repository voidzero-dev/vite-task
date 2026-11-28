mod macos_artifacts;

use std::io;

use fspy_shared::ipc::{IpcPath, IpcStr, PathAccess, channel::channel};
use fspy_shared_unix::macos::{EncodedPayload, Payload};
use futures_util::FutureExt as _;
use tokio_util::sync::CancellationToken;
use vt_materialized_artifact::{Artifact, artifact};

use crate::{
    ChildTermination, Command, TrackedChild, UntrackedReason, arena::PathAccessArena,
    error::SpawnError, ipc::ChannelAccesses,
};

#[derive(Debug)]
#[expect(clippy::struct_field_names, reason = "each field names a distinct injected path")]
struct MacosArtifacts {
    bash_path: Box<IpcStr>,
    coreutils_path: Box<IpcStr>,
    preload_path: Box<IpcStr>,
}

#[derive(Debug)]
pub struct SpyImpl {
    /// Materialized preload/helper artifacts, or `None` when nested inside an
    /// active outer `fspy` tracker (in which case spawns run untracked so the
    /// outer tracker captures the subtree).
    artifacts: Option<MacosArtifacts>,
}

impl SpyImpl {
    /// Materializes `fspy_preload_macos`, `oils_for_unix`, and `coreutils` into
    /// the current user's `vp_run` folder in the system temp dir.
    pub fn init() -> io::Result<Self> {
        const PRELOAD_CDYLIB: Artifact =
            artifact!("fspy_preload", "CARGO_CDYLIB_FILE_FSPY_PRELOAD_MACOS");

        if crate::nested::is_nested() {
            tracing::debug!(
                "nested inside an active fspy tracker; delegating tracking to outer tracker"
            );
            return Ok(Self { artifacts: None });
        }

        let preload_cdylib_path =
            PRELOAD_CDYLIB.materialize().suffix(".dylib").at_user_temp_dir("vp_run")?;

        Ok(Self {
            artifacts: Some(MacosArtifacts {
                preload_path: preload_cdylib_path.as_path().into(),
                bash_path: macos_artifacts::OILS_BINARY
                    .materialize()
                    .executable()
                    .at_user_temp_dir("vp_run")?
                    .as_path()
                    .into(),
                coreutils_path: macos_artifacts::COREUTILS_BINARY
                    .materialize()
                    .executable()
                    .at_user_temp_dir("vp_run")?
                    .as_path()
                    .into(),
            }),
        })
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
        let Some(artifacts) = &self.artifacts else {
            let mut tokio_command = command.into_tokio_command();
            let mut child = tokio_command.spawn().map_err(SpawnError::OsSpawn)?;
            return Ok(TrackedChild {
                stdin: child.stdin.take(),
                stdout: child.stdout.take(),
                stderr: child.stderr.take(),
                wait_handle: tokio::spawn(async move {
                    let status = tokio::select! {
                        status = child.wait() => status?,
                        () = cancellation_token.cancelled() => {
                            child.start_kill()?;
                            child.wait().await?
                        }
                    };
                    io::Result::Ok(ChildTermination {
                        status,
                        path_accesses: Err(UntrackedReason::Nested),
                    })
                })
                .map(|f| f?)
                .boxed(),
            });
        };

        let ipc_receiver = channel(crate::ipc::shm_capacity(), allocator_api2::alloc::Global)
            .map_err(SpawnError::ChannelCreation)?;

        let payload = Payload {
            ipc_channel_conf: ipc_receiver.conf(),
            preload_path: &artifacts.preload_path,
            bash_path: &artifacts.bash_path,
            coreutils_path: &artifacts.coreutils_path,
        };

        let payload_bump = bumpalo::Bump::new();
        let encoded_payload = EncodedPayload::encode(payload, &payload_bump);

        let mut exec = command.get_exec();
        let mut exec_resolve_accesses = PathAccessArena::default();
        encoded_payload
            .prepare_exec(&mut exec, |path| {
                exec_resolve_accesses.add(PathAccess::Read(IpcPath::from_bytes(path.as_ref())));
            })
            .map_err(|err| SpawnError::OsSpawn(io::Error::from_raw_os_error(err.raw_os_error())))?;
        command.set_exec(exec);
        command.env("FSPY", "1");

        let mut tokio_command = command.into_tokio_command();
        let mut child = tokio_command.spawn().map_err(SpawnError::OsSpawn)?;

        Ok(TrackedChild {
            stdin: child.stdin.take(),
            stdout: child.stdout.take(),
            stderr: child.stderr.take(),
            wait_handle: tokio::spawn(async move {
                let status = tokio::select! {
                    status = child.wait() => status?,
                    () = cancellation_token.cancelled() => {
                        child.start_kill()?;
                        child.wait().await?
                    }
                };

                let path_accesses = ChannelAccesses::try_from(ipc_receiver)
                    .map(|ipc_accesses| PathAccessIterable { exec_resolve_accesses, ipc_accesses });
                io::Result::Ok(ChildTermination { status, path_accesses })
            })
            .map(|f| f?)
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
        let accesses_in_arena = self.exec_resolve_accesses.borrow_accesses().iter().copied();
        let accesses_in_shm = self.ipc_accesses.iter_path_accesses();
        accesses_in_arena.chain(accesses_in_shm)
    }
}
