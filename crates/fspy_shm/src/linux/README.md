# Linux backend

The Linux backend stores data in a sealed `memfd`. A process opens the mapping by connecting to an abstract Unix-domain socket and receiving the descriptor from a broker. It then accesses the mapped memory directly.

The backend must avoid `/dev/shm` quotas, expose a large mapping without allocating every page up front, support synchronous opens from preload code, and stop new opens when the owner is dropped without invalidating existing views.

## Options considered

| Option                         | Decision                                                                                                                  |
| ------------------------------ | ------------------------------------------------------------------------------------------------------------------------- |
| POSIX shared memory            | Rejected because Linux stores it in `/dev/shm`, so it shares that mount's size limit.                                     |
| System V shared memory         | Rejected because IPC namespace limits affect availability and the owner must explicitly remove the segment.               |
| Sparse temporary file          | Rejected because dirty pages may reach disk, and sharing the path and deleting the file require additional handling.      |
| `memfd` with descriptor broker | Selected. It avoids `/dev/shm` and System V limits. The kernel keeps it alive while a descriptor or mapping refers to it. |

The broker accepts and serves clients with Tokio. Opening is synchronous because it can run before `main`, so creating an owner must occur inside a Tokio runtime.

## Why not `shared_memory`

`shared_memory` uses POSIX `shm_open` on Linux and cannot construct a mapping from a `memfd`, so it retains the `/dev/shm` dependency.

## Lifetime semantics

Dropping the owner stops the broker. Existing views remain valid; later opens fail.
