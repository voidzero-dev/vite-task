# Linux backend

The Linux backend stores data in a sealed `memfd`. An abstract Unix-domain
socket broker distributes the descriptor when a process opens the mapping;
the data path remains memory-mapped.

The design must avoid `/dev/shm` quotas, allocate a large logical mapping on
demand, support synchronous opens from preload code, and stop new opens when
the owner is dropped without invalidating existing views.

## Options considered

| Option                         | Decision                                                                                                            |
| ------------------------------ | ------------------------------------------------------------------------------------------------------------------- |
| POSIX shared memory            | Rejected because Linux backs it with `/dev/shm`.                                                                    |
| System V shared memory         | Rejected because IPC namespace quotas affect availability and segment lifetime is not tied to descriptor ownership. |
| Sparse temporary file          | Rejected because dirty pages may reach disk and the file needs a separate lifetime protocol.                        |
| `memfd` with descriptor broker | Selected. It avoids mount and IPC quotas while retaining descriptor-based lifetime.                                 |

The broker accepts and serves clients asynchronously. The client is
synchronous because it can run before `main`; creating an owner therefore
requires an ambient Tokio runtime only on the broker side.

## Why not `shared_memory`

`shared_memory` uses POSIX `shm_open` on Linux and cannot construct a mapping
from a `memfd`, so it retains the `/dev/shm` dependency.

## Lifetime semantics

Dropping the owner stops the broker. Existing views remain valid; later opens
fail.
