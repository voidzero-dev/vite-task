#[cfg(target_os = "linux")]
mod syscall_handler;

#[cfg(target_os = "macos")]
mod macos_artifacts;

use std::{io, path::Path};

#[cfg(target_os = "linux")]
use fspy_seccomp_unotify::supervisor::{SeccompNotifyHandler, handler::Sysno, supervise_with};
use fspy_shared::ipc::PathAccess;
#[cfg(not(target_env = "musl"))]
use fspy_shared::ipc::{NativeStr, channel::channel};
#[cfg(target_os = "macos")]
use fspy_shared_unix::payload::Artifacts;
#[cfg(not(target_env = "musl"))]
use fspy_shared_unix::payload::CallbackConf;
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
use crate::callback::unix::UnixCallbackServer;
#[cfg(not(target_env = "musl"))]
use crate::ipc::{OwnedReceiverLockGuard, SHM_CAPACITY};
use crate::{ChildTermination, Command, TrackedChild, arena::PathAccessArena, error::SpawnError};

#[cfg(target_os = "linux")]
fn syscalls_without(exclude: Sysno) -> Vec<Sysno> {
    SyscallHandler::syscalls().iter().copied().filter(|syscall| *syscall != exclude).collect()
}

#[derive(Debug)]
pub struct SpyImpl {
    #[cfg(target_os = "macos")]
    artifacts: Artifacts,

    #[cfg(not(target_env = "musl"))]
    preload_path: Box<NativeStr>,
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

            const PRELOAD_CDYLIB: Artifact = artifact!("fspy_preload");

            let preload_cdylib_path = PRELOAD_CDYLIB.materialize().suffix(".dylib").at(dir)?;
            preload_cdylib_path.as_path().into()
        };

        Ok(Self {
            #[cfg(not(target_env = "musl"))]
            preload_path,
            #[cfg(target_os = "macos")]
            artifacts: {
                let coreutils_path =
                    macos_artifacts::COREUTILS_BINARY.materialize().executable().at(dir)?;
                let bash_path = macos_artifacts::OILS_BINARY.materialize().executable().at(dir)?;
                Artifacts {
                    bash_path: bash_path.as_path().into(),
                    coreutils_path: coreutils_path.as_path().into(),
                }
            },
        })
    }

    #[expect(
        clippy::too_many_lines,
        reason = "spawn orchestrates the supervisor, callback server, payload and child setup"
    )]
    pub(crate) async fn spawn(
        &self,
        mut command: Command,
        cancellation_token: CancellationToken,
    ) -> Result<TrackedChild, SpawnError> {
        let file_callback = command.file_callback.take();

        // Supervisor side of the optional blocking open/close callback for the
        // preload backend (Linux glibc + macOS).
        #[cfg(not(target_env = "musl"))]
        let callback_server = file_callback
            .as_ref()
            .map(|callback| UnixCallbackServer::new(callback.callback.clone()))
            .transpose()
            .map_err(SpawnError::CallbackChannelCreation)?;

        #[cfg(target_os = "linux")]
        let supervisor = file_callback
            .clone()
            .map_or_else(
                // No callback: keep `close` out of the seccomp filter entirely
                // so there is no per-close overhead.
                || supervise_with(SyscallHandler::default, &syscalls_without(Sysno::close)),
                // A callback also intercepts `close`, and is injected into
                // each per-process syscall handler.
                |callback| {
                    supervise_with(
                        move || SyscallHandler::with_callback(Some(callback.clone())),
                        SyscallHandler::syscalls(),
                    )
                },
            )
            .map_err(SpawnError::Supervisor)?;

        #[cfg(not(target_env = "musl"))]
        let (ipc_channel_conf, ipc_receiver) =
            channel(SHM_CAPACITY).map_err(SpawnError::ChannelCreation)?;

        #[cfg(not(target_env = "musl"))]
        let callback_conf = match (callback_server.as_ref(), file_callback.as_ref()) {
            (Some(server), Some(callback)) => Some(CallbackConf {
                socket_path: server.socket_path().as_os_str().into(),
                mask: callback.mask,
            }),
            _ => None,
        };

        let payload = Payload {
            #[cfg(not(target_env = "musl"))]
            ipc_channel_conf,

            #[cfg(target_os = "macos")]
            artifacts: self.artifacts.clone(),

            #[cfg(not(target_env = "musl"))]
            preload_path: self.preload_path.clone(),

            #[cfg(not(target_env = "musl"))]
            callback: callback_conf,

            #[cfg(target_os = "linux")]
            seccomp_payload: supervisor.payload().clone(),
        };

        let encoded_payload = encode_payload(payload);

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
            // because we need to stop the supervisor and lock the channel as soon as the child exits.
            wait_handle: tokio::spawn(async move {
                let status = tokio::select! {
                    status = child.wait() => status?,
                    () = cancellation_token.cancelled() => {
                        child.start_kill()?;
                        child.wait().await?
                    }
                };

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

                // Drain in-flight blocking callbacks before locking the channel.
                #[cfg(not(target_env = "musl"))]
                if let Some(callback_server) = callback_server {
                    callback_server.shutdown().await;
                }

                // Lock the ipc channel after the child has exited.
                // We are not interested in path accesses from descendants after the main child has exited.
                #[cfg(not(target_env = "musl"))]
                let ipc_receiver_lock_guard =
                    OwnedReceiverLockGuard::lock_async(ipc_receiver).await?;
                let path_accesses = PathAccessIterable {
                    arenas,
                    #[cfg(not(target_env = "musl"))]
                    ipc_receiver_lock_guard,
                };

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
    ipc_receiver_lock_guard: OwnedReceiverLockGuard,
}

impl PathAccessIterable {
    pub fn iter(&self) -> impl Iterator<Item = PathAccess<'_>> {
        let accesses_in_arena =
            self.arenas.iter().flat_map(|arena| arena.borrow_accesses().iter()).copied();

        #[cfg(not(target_env = "musl"))]
        {
            let accesses_in_shm = self.ipc_receiver_lock_guard.iter_path_accesses();
            accesses_in_shm.chain(accesses_in_arena)
        }
        #[cfg(target_env = "musl")]
        {
            accesses_in_arena
        }
    }
}
