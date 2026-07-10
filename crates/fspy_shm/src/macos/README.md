# macOS backend

The macOS backend uses named POSIX shared memory. Another process opens the same bytes using the POSIX name. Pages are allocated as they are accessed, and dropping the owner removes the name. No broker is required.

## Options considered

| Option                               | Decision                                                                                                           |
| ------------------------------------ | ------------------------------------------------------------------------------------------------------------------ |
| System V shared memory               | Rejected because kernel IPC limits affect availability and the owner must explicitly remove the segment.           |
| Sparse temporary file                | Rejected because dirty pages may reach disk. Another process needs the path, and the owner must delete the file.   |
| Mach memory entry with port transfer | Rejected because another process can receive the memory entry only through a Mach port, which requires a broker.   |
| POSIX shared memory                  | Selected. Another process can open it by name, pages are allocated as accessed, and `shm_unlink` removes the name. |

Unlike Linux, macOS does not route POSIX shared memory through a container's `/dev/shm` mount.

## Why not `shared_memory`

`shared_memory` calls the same POSIX APIs. Fspy only needs to create, open, map, and unlink shared memory, so the backend calls those APIs directly.

## Lifetime semantics

Dropping the owner unlinks the POSIX name. Existing views remain valid; later opens fail.
