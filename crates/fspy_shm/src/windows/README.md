# Windows backend

The Windows backend maps a sparse temporary file through a named section. This
provides name-based opens and a large logical mapping without charging its full
size to system commit or disk allocation.

## Options considered

| Option                                                       | Decision                                                                                                                            |
| ------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------- |
| Paging-file-backed section                                   | Rejected because the section's full size is charged against system commit.                                                          |
| Reserved section with incremental `VirtualAlloc(MEM_COMMIT)` | Rejected because concurrent writers would need a shared commit frontier and refill protocol.                                        |
| Sparse temporary file with a named section                   | Selected. Untouched ranges consume neither disk allocation nor commit, and the section name provides process-independent discovery. |

[`FILE_ATTRIBUTE_TEMPORARY`](https://learn.microsoft.com/windows/win32/api/fileapi/ns-fileapi-createfile2_extended_parameters)
favors cache retention but does not prevent writeback under memory pressure.
[Sparseness](https://learn.microsoft.com/windows/win32/fileio/sparse-files)
avoids allocation only for untouched ranges. Creation fails if the temporary
volume does not support sparse files.

## Why not `shared_memory`

`shared_memory` creates and extends a regular backing file before callers can
mark it sparse, so it cannot provide the required allocation behavior.

`shared_memory` also uses the backing-file path as part of its persistence
protocol. Fspy uses the section name for discovery so an opener does not need
the creator's temporary-directory path.

## Lifetime semantics

Dropping the owner unmaps its view and closes the delete-on-close backing file.
Existing views keep the section alive. The section name may remain openable
during that period, so `ChannelConf::sender` uses the receiver's lock file as
the authoritative lifetime gate.
