#[cfg(target_os = "linux")]
mod syscall_handler;

#[cfg(target_os = "macos")]
mod macos_artifacts;

use std::{io, path::Path};

#[cfg(target_os = "linux")]
use fspy_seccomp_unotify::supervisor::supervise_with_handler;
use fspy_shared::ipc::PathAccess;
#[cfg(not(target_env = "musl"))]
use fspy_shared::ipc::{IpcStr, channel::channel};
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

#[cfg(not(target_env = "musl"))]
use crate::ipc::ChannelAccesses;
use crate::{ChildTermination, Command, TrackedChild, arena::PathAccessArena, error::SpawnError};

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
        let observer = command.access_observer.take();
        let kill_process_tree = command.kill_process_tree;
        #[cfg(target_os = "linux")]
        let supervisor = {
            let observer = observer.clone();
            supervise_with_handler(move || SyscallHandler::with_observer(observer.clone()))
                .map_err(SpawnError::Supervisor)?
        };

        #[cfg(not(target_env = "musl"))]
        let ipc_receiver = channel(crate::ipc::shm_capacity(), allocator_api2::alloc::Global)
            .map_err(SpawnError::ChannelCreation)?;

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
                let access = PathAccess { mode, path: path.into() };
                if let Some(observer) = &observer {
                    observer(Ok(access));
                }
                exec_resolve_accesses.add(access);
            },
        )
        .map_err(|err| SpawnError::Injection(err.into()))?;
        command.set_exec(exec);
        command.env("FSPY", "1");

        let mut tokio_command = command.into_tokio_command();
        if kill_process_tree {
            tokio_command.process_group(0);
        }

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
                let wait = wait_child(&mut child, &cancellation_token, kill_process_tree);
                #[cfg(not(target_env = "musl"))]
                let status =
                    crate::ipc::wait_observed(&ipc_receiver, observer.as_ref(), wait).await?;
                #[cfg(target_env = "musl")]
                let status = wait.await?;

                let arenas = std::iter::once(exec_resolve_accesses);
                // Stop the supervisor and collect path accesses from it.
                #[cfg(target_os = "linux")]
                let arenas = arenas.chain(
                    supervisor
                        .stop()
                        .await?
                        .into_iter()
                        .map(syscall_handler::SyscallHandler::into_arena),
                );
                let arenas = arenas.collect::<Vec<_>>();

                // Close the ipc channel after the child has exited.
                // We are not interested in path accesses from descendants after the main child has exited.
                #[cfg(not(target_env = "musl"))]
                let path_accesses = ChannelAccesses::try_from(ipc_receiver)
                    .map(|ipc_accesses| PathAccessIterable { arenas, ipc_accesses });
                #[cfg(target_env = "musl")]
                let path_accesses = Ok(PathAccessIterable { arenas });

                io::Result::Ok(ChildTermination { status, path_accesses })
            })
            .map(|f| f?) // flatten JoinError and io::Result
            .boxed(),
        })
    }
}

pub struct PathAccessIterable {
    arenas: Vec<PathAccessArena>,
    #[cfg(not(target_env = "musl"))]
    ipc_accesses: ChannelAccesses,
}

impl PathAccessIterable {
    pub fn iter(&self) -> impl Iterator<Item = PathAccess<'_>> {
        let accesses_in_arena =
            self.arenas.iter().flat_map(|arena| arena.borrow_accesses().iter()).copied();

        #[cfg(not(target_env = "musl"))]
        {
            let accesses_in_shm = self.ipc_accesses.iter_path_accesses();
            accesses_in_shm.chain(accesses_in_arena)
        }
        #[cfg(target_env = "musl")]
        {
            accesses_in_arena
        }
    }
}

async fn wait_child(
    child: &mut tokio::process::Child,
    cancellation: &CancellationToken,
    tree: bool,
) -> io::Result<std::process::ExitStatus> {
    let process_id = child.id();
    let status = tokio::select! {
        status = child.wait() => status?,
        () = cancellation.cancelled() => {
            if tree {
                // Nested persistent runners need a chance to clean up their
                // own process groups before their parent is terminated.
                signal_group(process_id, libc::SIGINT);
                if let Ok(status) = tokio::time::timeout(std::time::Duration::from_secs(1), child.wait()).await {
                    status?
                } else {
                    signal_group(process_id, libc::SIGKILL);
                    child.wait().await?
                }
            } else {
                child.start_kill()?;
                child.wait().await?
            }
        }
    };
    if tree && signal_group(process_id, libc::SIGINT) {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
        while signal_group(process_id, 0) && tokio::time::Instant::now() < deadline {
            // Grandchildren are not waitable children of this process. Unix
            // has no completion notification for an entire process group.
            #[expect(
                clippy::disallowed_methods,
                reason = "poll termination of non-child group members"
            )]
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        signal_group(process_id, libc::SIGKILL);
    }
    Ok(status)
}

fn signal_group(process_id: Option<u32>, signal: i32) -> bool {
    process_id.and_then(|id| i32::try_from(id).ok()).is_some_and(|process_id| {
        // SAFETY: the child was placed in its own group before exec. A negative
        // PID targets only that group, including descendants holding pipes.
        unsafe { libc::kill(-process_id, signal) == 0 }
    })
}
