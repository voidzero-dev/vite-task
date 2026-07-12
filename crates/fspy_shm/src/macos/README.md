# macOS backend

The macOS backend uses named POSIX shared memory. Another process opens the same object by name. Pages are allocated as they are accessed, and dropping the owner removes the name. Fspy does not need a separate service to pass file descriptors between processes.

## Options considered

| Option                               | Decision                                                                                                                                          |
| ------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------- |
| System V shared memory               | Rejected because kernel IPC limits affect availability and the owner must explicitly remove the segment.                                          |
| Sparse temporary file                | Rejected because dirty pages may reach disk. Another process needs the path, and the owner must delete the file.                                  |
| Mach memory entry with port transfer | Rejected because another process can receive the memory entry only through a Mach port. Fspy would need a separate service to transfer that port. |
| POSIX shared memory                  | Selected. Another process can open it by name, pages are allocated as accessed, and `shm_unlink` removes the name.                                |

Unlike Linux, macOS does not route POSIX shared memory through a container's `/dev/shm` mount.

## Why not `shared_memory`

`shared_memory` uses the same POSIX shared-memory mechanism, but it also supports opening mappings through files and changing which process deletes them. Fspy needs neither feature. It only needs to create, open, map, and unlink shared memory, so the backend calls the POSIX APIs directly.

## Lifetime semantics

Dropping the owner unlinks the POSIX name. Existing views remain valid; later opens fail.
