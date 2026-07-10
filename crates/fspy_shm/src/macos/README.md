# macOS backend

The macOS backend uses named POSIX shared memory. It provides name-based opens,
demand-backed storage, and unlink-on-owner-drop semantics without a broker.

## Options considered

| Option                               | Decision                                                                                                  |
| ------------------------------------ | --------------------------------------------------------------------------------------------------------- |
| System V shared memory               | Rejected because kernel IPC limits affect availability and segment lifetime requires a separate protocol. |
| Sparse temporary file                | Rejected because dirty pages may reach disk and the file needs a separate lifetime protocol.              |
| Mach memory entry with port transfer | Rejected because name-based discovery would require a port broker.                                        |
| POSIX shared memory                  | Selected. It provides naming, demand-backed storage, and unlink semantics directly.                       |

Unlike Linux, macOS does not route POSIX shared memory through a container's
`/dev/shm` mount.

## Why not `shared_memory`

`shared_memory` opens the size reported by `fstat` instead of validating the
caller's expected size. Darwin rounds the object size to a VM page, so that API
cannot preserve fspy's exact logical-size contract.

## Lifetime semantics

Dropping the owner unlinks the POSIX name. Existing views remain valid; later
opens fail.
