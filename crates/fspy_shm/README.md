# `fspy_shm`

`fspy_shm` is the private shared-memory layer used by fspy IPC channels. It gives the channel one API for creating a mapping at a caller-chosen path, opening additional views of the same bytes from any process that knows the path, and removing the backing file.

`fspy_shm` exposes only the operations used by fspy. The caller owns the path: it decides where the backing file lives, passes the same path to every process that opens the shared memory, and removes it when the shared memory is no longer needed. The fspy channel generates an absolute uniquely-named path in the system temporary directory and holds it in a keeper that removes it on drop.

## API

The public API is defined in [`src/lib.rs`](src/lib.rs).

| API                   | Contract                                                                                |
| --------------------- | --------------------------------------------------------------------------------------- |
| `create(path, size)`  | Creates a zero-initialized backing file at `path` and returns an opened `ShmHandle`.    |
| `open(path)`          | Opens an `ShmHandle` on the shared memory backed by the file at `path`.                 |
| `remove(path)`        | Removes the backing file. Later opens fail; existing handles and mappings keep working. |
| `ShmHandle::map()`    | Maps the shared bytes. Callable more than once.                                         |
| `Mapping::len()`      | Returns the mapped size.                                                                |
| `Mapping::as_ptr()`   | Returns a mutable raw pointer to the first byte.                                        |
| `Mapping::as_slice()` | Returns the bytes as a shared slice. The caller must prevent mutation for its lifetime. |

`ShmHandle` is the opened file: `create` returns one so the creator never looks its own file up by path, and `open` returns one to everybody else. `Mapping` is the bytes: it keeps them alive until dropped and can do nothing else. Neither synchronizes memory access. The fspy channel adds that on top with atomic frame headers and a lock file: senders hold a shared file lock while writing, and the receiver takes the exclusive lock before reading, which waits for existing senders and rejects new ones.

Every byte in a mapping returned by `create` is initially zero. `open` exposes the mapping's current contents and does not reinitialize them.

## Implementation

One implementation serves every platform: a sparse file at the caller's path. Another process opens the mapping by opening that path. There is no broker, no global object name, and no asynchronous runtime.

Only written pages ever occupy memory or disk. The multi-gigabyte capacity fspy asks for therefore costs about as much as the data a run actually records.

Every operation goes through [`fspy_nostd`](../fspy_nostd) wrappers or direct Win32 calls. The platform-specific parts are three short passages:

| Concern          | Unix                                     | Windows                                                                     |
| ---------------- | ---------------------------------------- | --------------------------------------------------------------------------- |
| Same-user access | `mode(0o600)` on the backing file        | the per-user `%TEMP%` ACL of the caller's chosen directory                  |
| Sparseness       | file holes, produced by setting a length | `FSCTL_SET_SPARSE` before setting a length, or NTFS allocates every cluster |
| Removal          | unlink the path                          | POSIX delete via `FileDispositionInfoEx`; see below                         |

`FILE_ATTRIBUTE_TEMPORARY` asks Windows to keep the data in memory when it can. Creation fails on a volume without sparse-file support.

`remove` unlinks the path on Unix. On Windows it relies on [POSIX delete semantics](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntddk/ns-ntddk-_file_disposition_information_ex), which requires NTFS on Windows 10 1607 or newer: the name goes away at once, while existing handles keep working and [mapped views keep the data alive](https://learn.microsoft.com/en-us/windows/win32/api/memoryapi/nf-memoryapi-createfilemappingw) until the last one goes away.

## Options considered

| Option                                    | Decision                                                                                                                                                                                        |
| ----------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| POSIX shared memory                       | Rejected. Coding-agent sandboxes deny it on macOS (`ipc-posix-shm-write-create`), and Linux stores the objects in `/dev/shm`, so they share that mount's size limit.                            |
| `memfd` with a descriptor broker          | Rejected. Handing the descriptor to another process needs a Unix-domain socket, which the same sandboxes block, and the broker needed an ambient asynchronous runtime just to create a mapping. |
| System V shared memory                    | Rejected. IPC namespace limits affect availability and the owner must explicitly remove the segment.                                                                                            |
| Paging-file-backed section                | Rejected. The section's full size is charged against the system commit limit.                                                                                                                   |
| Reserved section with incremental commits | Rejected. Writers would have to coordinate when committing more pages.                                                                                                                          |
| Mach memory entry with port transfer      | Rejected. Another process can receive the memory entry only through a Mach port, which needs a separate service.                                                                                |
| Sparse temporary file, opened by path     | Selected. Every sandbox fspy runs under permits it, no descriptor has to be passed, and all platforms end up with the same lifetime semantics.                                                  |

Earlier revisions rejected temporary files because dirty pages can reach disk. Only pages fspy writes are dirty, about a hundred kilobytes per run, so writeback costs little. Unlike POSIX shared memory on Linux, a temporary file does not compete for the `/dev/shm` mount's size limit.

## Why not `shared_memory`

`shared_memory` uses POSIX `shm_open` on Unix, which keeps the sandbox and `/dev/shm` problems above. On Windows it creates and extends a regular file before fspy could mark it sparse, so untouched ranges would still consume disk blocks.

## Lifetime semantics

- While the backing file exists, a process that knows the path can open the shared memory.
- `remove` deletes the backing file's name, so later opens fail. This is cleanup, not a stop signal: processes that already opened the shared memory keep reading and writing. The fspy channel stops writers with the close gate it stores in the shared bytes.
- An `ShmHandle` and its `Mapping`s stay usable after the backing file is removed. They keep the bytes alive and cannot restore the path.

The channel guards the same window from its own side: [`ChannelConf::sender`](../fspy_shared/src/ipc/channel/mod.rs) opens and locks the receiver's exact lock-file path before it calls `fspy_shm::open`, and the receiver removes that path before removing the backing file, so a sender that starts later fails before opening shared memory.

If the process that owns the path is killed before it calls `remove`, the file stays behind: on Unix for the system's temporary-file reaper, on Windows until a cleanup tool runs. The file costs about as much disk as the run wrote into it.
