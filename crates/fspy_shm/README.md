# `fspy_shm`

`fspy_shm` is the private shared-memory layer used by fspy IPC channels. It gives the channel one API for creating a mapping, passing its identifier to another process, and opening additional views of the same bytes.

`fspy_shm` exposes only the operations used by fspy. Treat an identifier as an opaque `OsStr`; do not depend on how it is built.

## API

The public API is defined in [`src/lib.rs`](src/lib.rs).

| API                 | Contract                                                                                |
| ------------------- | --------------------------------------------------------------------------------------- |
| `create(size)`      | Creates a zero-initialized backing file and returns its `ShmKeeper` and an `ShmHandle`. |
| `open(id)`          | Opens an `ShmHandle` on the shared memory identified by `id`.                           |
| `ShmKeeper::id()`   | Returns the identifier another process passes to `open`.                                |
| `ShmHandle::map()`  | Maps the shared bytes. Callable more than once.                                         |
| `Mapping::len()`    | Returns the mapped size.                                                                |
| `Mapping::as_ptr()` | Returns a mutable raw pointer to the first byte.                                        |

`ShmKeeper` is the name: while it lives, `open` succeeds, and dropping it removes the backing file. `ShmHandle` is the opened file: `create` returns one so the creator never looks its own file up by name, and `open` returns one to everybody else. `Mapping` is the bytes: it keeps them alive until dropped and can do nothing else. None of the three synchronizes memory access or dereferences the mapping, so the crate has no safety conditions of its own. All access rules live one layer up, in the fspy channel: a single atomic end-offset word hands every writer a frame range of its own, and a close gate stored in the same mapping tracks writers. Every writer holds a gate guard while it writes. The receiver closes the gate with one atomic operation, which refuses every later writer and reports whether any write was still in flight.

Every byte in a mapping returned by `create` is initially zero. `open` exposes the mapping's current contents and does not reinitialize them.

## Implementation

One implementation serves every platform: a sparse file named `vite-task-fspy-<uuid>.shm` directly in the system temporary directory. The identifier is the file's absolute path, so another process opens the mapping by opening that path. There is no broker, no global object name, and no asynchronous runtime. The files sit in the temporary directory itself rather than a shared subdirectory: a subdirectory would belong to whichever user created it first and block everyone else, while uniquely named `0o600` files in a sticky-bit directory work for all users.

Only written pages ever occupy memory or disk. The multi-gigabyte capacity fspy asks for therefore costs about as much as the data a run actually records.

Mapping goes through `memmap2` on every platform. The remaining platform-specific parts are three short passages:

| Concern           | Unix                                     | Windows                                                                     |
| ----------------- | ---------------------------------------- | --------------------------------------------------------------------------- |
| Same-user access  | `mode(0o600)` on the backing file        | the per-user `%TEMP%` ACL                                                   |
| Sparseness        | file holes, produced by setting a length | `FSCTL_SET_SPARSE` before setting a length, or NTFS allocates every cluster |
| Keeper cleanup    | unlink the path                          | unlink the path; see the fallback below                                     |
| Descriptor safety | `O_CLOEXEC`, the Rust standard default   | non-inheritable handles, the Rust standard default                          |

`FILE_ATTRIBUTE_TEMPORARY` asks Windows to keep the data in memory when it can. Creation fails on a volume without sparse-file support.

The keeper removes the name with `remove_file` on every platform. Modern Windows deletes with POSIX semantics: the name goes away at once, while [existing handles keep working](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntddk/ns-ntddk-_file_disposition_information_ex) and [mapped views keep the data alive](https://learn.microsoft.com/en-us/windows/win32/api/memoryapi/nf-memoryapi-createfilemappingw) until the last one goes away. The first page also reserves the right to fail the delete while a mapped view exists, and Windows versions without POSIX delete do fail it. The keeper then falls back to reopening the file with `FILE_FLAG_DELETE_ON_CLOSE` and closing it, which deletes the file once every handle to it is closed.

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

`create` returns the only keeper. `open` returns `ShmHandle`s.

- While the keeper is alive, a process that knows the identifier can open the shared memory.
- Dropping the keeper removes the backing file's name, so later opens fail. This is cleanup, not a stop signal: processes that already opened the shared memory keep reading and writing. The fspy channel stops writers with the close gate it stores in the shared bytes.
- An `ShmHandle` and its `Mapping`s stay usable after the keeper is gone. They keep the bytes alive and cannot extend the identifier's validity.

The channel builds on that: [`ChannelConf::sender`](../fspy_shared/src/ipc/channel/mod.rs) is just an `open`, and the receiver drops the keeper when it closes the channel. A sender that attached earlier keeps its mapping, but the close gate refuses its claims from then on.

If the keeper's process is killed, its `Drop` never runs and the file stays behind: on Unix for the system's temporary-file reaper, on Windows until a cleanup tool runs. The file costs about as much disk as the run wrote into it.
