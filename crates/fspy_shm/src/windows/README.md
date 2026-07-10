# Windows backend

The Windows backend maps a sparse temporary file and gives the mapping a Windows section name. Another process opens the same section using only that name. Creating the mapping does not reserve its full size in system commit or disk space.

## Options considered

| Option                                                       | Decision                                                                                                                      |
| ------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------- |
| Paging-file-backed section                                   | Rejected because the section's full size is charged against system commit.                                                    |
| Reserved section with incremental `VirtualAlloc(MEM_COMMIT)` | Rejected because writers would need to coordinate when committing more pages.                                                 |
| Sparse temporary file with a named section                   | Selected. Disk blocks are allocated only for written ranges, and another process can open the mapping using the section name. |

`FILE_ATTRIBUTE_TEMPORARY` tells Windows to keep file data in memory when possible, but Windows may still write dirty pages to disk under memory pressure. Creation fails if the temporary volume does not support sparse files.

## Why not `shared_memory`

`shared_memory` creates and extends a regular file before fspy can mark it sparse. It therefore cannot ensure that untouched ranges use no disk blocks.

`shared_memory` also uses the file path to identify the mapping. Fspy uses the section name, so another process does not need the creator's temporary-file path.

## Lifetime semantics

Dropping the owner unmaps its view and closes the delete-on-close backing file. Existing views keep the section alive. The section name may remain openable during that period. `ChannelConf::sender` checks the receiver's lock file first to prevent new senders after the receiver has shut down.
