# IPC Transport

Cross-platform IPC straight on top of OS primitives — no `interprocess` or other transport-abstraction crate in between:

| Platform           | Server                                                               | Client                                                     |
| ------------------ | -------------------------------------------------------------------- | ---------------------------------------------------------- |
| Unix (macOS/Linux) | `tokio::net::UnixListener` (path inside a `tempfile::NamedTempFile`) | `std::os::unix::net::UnixStream`                           |
| Windows            | `tokio::net::windows::named_pipe::NamedPipeServer` (per-instance)    | `std::fs::OpenOptions::open` on the `\\.\pipe\<name>` path |

The socket path or pipe name is passed to the task process via an env var shared between `vite_task_server` and `vite_task_client` (the specific name is an implementation detail). Clients check for its presence and skip IPC gracefully if absent.

## Server Model

One listener per task execution. The runner creates a new socket just before spawning the task and tears it down after the task exits.

The listener runs an accept loop and handles multiple concurrent clients — build tools may spawn worker processes or threads that each connect independently.

On Windows, each accepted connection consumes the pending `NamedPipeServer` instance, and the accept loop immediately creates a fresh instance for the next client. There is a brief window during the swap where a client can see `ERROR_PIPE_BUSY`; the client retries (bounded) until the new instance is ready.

Platform differences are handled via `#[cfg(unix)]` / `#[cfg(windows)]`.
