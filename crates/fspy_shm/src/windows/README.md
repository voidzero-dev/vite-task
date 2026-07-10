# Windows backend

The Windows backend maps a sparse temporary file through a named Windows
section object. The sparse file avoids full disk allocation, while the named
section lets another process open the same bytes without knowing the backing
file path.

## Requirements

The fspy channel needs a large logical mapping with these properties:

- creation must not consume the full mapping in physical memory, system
  commit, or disk allocation,
- another process must be able to open it from a serialized identifier,
- concurrent writers must use memory and atomic operations instead of a
  syscall for every record,
- opening must not depend on the creator's temporary-directory environment,
  and
- existing views must survive owner cleanup.

## Options considered

| Option                                                       | Decision                                                                                                                                          |
| ------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------- |
| Paging-file-backed named mapping                             | Rejected. The default committed section reserves the full mapping against the system commit limit even though physical pages remain demand-paged. |
| Reserved section with incremental `VirtualAlloc(MEM_COMMIT)` | Rejected. It needs a shared committed frontier, chunk-refill coordination, and boundary handling across concurrent writers.                       |
| Named pipe or Unix-domain socket data path                   | Rejected. Every record would require a syscall and buffer management under concurrent writes.                                                     |
| Regular temporary file mapping                               | Rejected. Extending a normal file cannot guarantee that the full logical size remains unallocated on disk.                                        |
| Sparse temporary file mapping                                | Selected. It preserves memory-style writes, allocates backing ranges on demand, and supports a native named section.                              |

The backing file is marked sparse before its logical length is established.
[`FILE_ATTRIBUTE_TEMPORARY`](https://learn.microsoft.com/windows/win32/api/fileapi/ns-fileapi-createfile2_extended_parameters)
asks Windows to retain modified pages in cache when possible. It does not
guarantee that dirty pages never reach disk under memory pressure.
[Sparseness](https://learn.microsoft.com/windows/win32/fileio/sparse-files)
prevents allocation for untouched ranges, but nonzero writes can allocate disk
space.

Sparse-file support is required. The backend returns the sparse-control error
instead of silently falling back to a fully allocated file. NTFS and `ReFS`
support sparse files; a temporary directory on an unsupported filesystem
cannot host the mapping.

## Why not `shared_memory`

The `shared_memory` Windows backend creates and extends its own regular backing
file. Its API does not expose that file before creating the mapping, so fspy
cannot mark it sparse before establishing the logical size. It cannot
guarantee the allocation behavior required by the channel.

`shared_memory` also uses the backing-file path as part of its persistence
protocol. Fspy openers may have a different `TEMP`, `TMP`, or working directory
from the creator. The selected backend uses the named section as the only
discovery mechanism and uses the channel's existing lock file as its lifetime
gate.

`memmap2` does not replace the missing behavior. On Windows it creates an
unnamed mapping from a file handle and has no public API for opening an
existing section by name.

## Lifetime semantics

Dropping the owner unmaps its view and closes the delete-on-close backing file.
Views opened by other processes remain valid and keep the section alive.

Windows may allow the section name to be opened while one of those views
remains. This does not bypass the channel lifetime. `ChannelConf::sender` must
first open the receiver's serialized lock-file path, and the receiver removes
that path before dropping the mapping owner.
