# `fspy_shm`

`fspy_shm` is the private shared-memory layer used by fspy IPC channels. It gives the channel one API for creating a mapping, passing its identifier to another process, and opening additional views of the same bytes.

`fspy_shm` exposes only the operations used by fspy. Callers must treat an identifier as a string and must not depend on a platform's naming scheme.

## API

The public API is defined in [`src/lib.rs`](src/lib.rs).

| API               | Contract                                                                           |
| ----------------- | ---------------------------------------------------------------------------------- |
| `create(size)`    | Creates a non-empty mapping and returns its unique owner.                          |
| `open(id)`        | Opens another view of the mapping identified by `id`.                              |
| `Shm::id()`       | Returns the identifier to send to another process.                                 |
| `Shm::len()`      | Returns the mapped size.                                                           |
| `Shm::as_ptr()`   | Returns a mutable raw pointer to the first byte.                                   |
| `Shm::as_slice()` | Returns a shared slice. The caller must prevent mutation for the slice's lifetime. |

`Shm` does not synchronize memory access. The fspy channel combines it with atomic frame headers and a lock file. Senders hold a shared file lock while writing. The receiver takes the exclusive lock before reading, which waits for existing senders and rejects new ones.

## Ownership semantics

`create` returns the only owner. `open` returns non-owning views.

- While the owner is alive, a process that knows the identifier can open the mapping.
- An opened view remains usable after the owner is dropped. Its operating system mapping keeps the underlying bytes alive.
- After the owner is dropped, new opens behave differently by platform. POSIX removes the name. Windows can continue accepting opens by section name until the final handle or view is closed.

The channel hides that difference with its lock file. [`ChannelConf::sender`](../fspy_shared/src/ipc/channel/mod.rs) opens and locks the receiver's exact lock-file path before it calls `fspy_shm::open`. The receiver removes that path before dropping the owner, so a sender that starts later fails before opening shared memory.

## Backend boundary

At this point in the stack, `fspy_shm` delegates mapping creation and opening to the [`shared_memory`](https://crates.io/crates/shared_memory) crate. Because callers use only the API above, later changes can replace the backend on each platform without changing the channel protocol.
