# Windows backend

The Windows backend creates a sparse temporary file and exposes it through a
named file-mapping object. The file supplies pageable backing storage. Other
processes discover the mapping by its kernel-object name and never open the
file path.

## Runtime flow

`create(size)` performs these steps:

1. Generate a random identifier and create a unique file below the creator's
   temporary directory.
2. Open the file read-write with read, write, and delete sharing, plus
   `FILE_ATTRIBUTE_TEMPORARY` and `FILE_FLAG_DELETE_ON_CLOSE`.
3. Mark the file sparse with `FSCTL_SET_SPARSE`.
4. Set its logical length without allocating the full file on disk.
5. Create a read-write mapping named
   `Local\vite-task-fspy-{identifier}-{size}`.
6. Map an exact-size view and return it with the backing-file handle.

`open(id, size)` derives the same object name, calls `OpenFileMappingW`, and
maps an exact-size view. It does not resolve a temporary directory, so a child
can change `TEMP`, `TMP`, or its working directory without losing access.

The mapping handle can close after `MapViewOfFile` succeeds. Windows keeps an
internal reference to the section for each mapped view.

## Requirements

The fspy channel needs a large logical mapping with these properties:

- creating it must not consume its full size in physical memory, system commit,
  or disk allocation,
- processes must open it from an opaque string identifier,
- concurrent writers must use ordinary memory and atomic operations instead
  of a syscall for every record,
- opening must not depend on the creator's temporary-directory environment,
  and
- existing views must survive owner cleanup.

## Options considered

| Option                                                       | Decision                                                                                                                                          |
| ------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------- |
| Paging-file-backed named mapping                             | Rejected. The default committed section reserves the full mapping against the system commit limit even though physical pages remain demand-paged. |
| Reserved section with incremental `VirtualAlloc(MEM_COMMIT)` | Rejected. It needs a shared committed frontier, chunk-refill coordination, and boundary handling across concurrent writers.                       |
| Named pipe or Unix-domain socket data path                   | Rejected. Every record would require a syscall and descriptor-buffer management under concurrent writes.                                          |
| Regular temporary file mapping                               | Rejected. Extending a normal file cannot guarantee that the full logical size remains unallocated on disk.                                        |
| Sparse temporary file mapping                                | Selected. It preserves memory-style writes, allocates backing ranges on demand, and supports a native named mapping object.                       |

[`FILE_ATTRIBUTE_TEMPORARY`](https://learn.microsoft.com/windows/win32/api/fileapi/ns-fileapi-createfile2_extended_parameters)
tells Windows to retain modified pages in cache when possible because the file
is short-lived. It does not guarantee that dirty pages never reach disk under
memory pressure. [Sparseness](https://learn.microsoft.com/windows/win32/fileio/sparse-files)
prevents allocation for untouched ranges; it does not turn a file mapping into
anonymous memory.

## Why not `shared_memory`

The `shared_memory` Windows backend creates and extends its own regular backing
file. Its API does not expose the file before `CreateFileMappingW`, so fspy
cannot mark it sparse before establishing the mapping size. It therefore
cannot guarantee the allocation behavior required by the channel.

The crate also implements persistence through a backing-file rendezvous.
Fspy already has a serialized lock-file path that gates sender creation, while
the Windows mapping must remain openable when a child has a different
temporary-directory environment. Reusing that persistence layer would add a
second lifecycle protocol without solving sparse allocation.

`memmap2` is not a replacement for this backend. On Windows it creates an
unnamed mapping from a file handle and does not provide a public API for
opening an existing mapping object by name.

## Lifetime

The owner retains the backing-file handle. Dropping it unmaps the owner's view
and closes the delete-on-close file. Views opened by other processes remain
valid and keep the section object alive.

While any view remains, Windows may still allow `OpenFileMappingW` to open the
section name after the owner has dropped. This does not bypass the fspy channel
lifetime: `ChannelConf::sender` must first open the receiver's lock-file path,
and the receiver removes that path before dropping the mapping owner.

The backend requires sparse-file support. It returns the `FSCTL_SET_SPARSE`
error instead of silently falling back to a fully allocated file. NTFS and
`ReFS` support sparse files; a temporary directory on an unsupported filesystem
cannot host this mapping.

## Verification

The module tests verify that:

- another process can open the mapping without sharing the creator's current
  directory or temporary-directory environment,
- dropping the owner deletes the backing file without invalidating an opened
  view,
- the production-size mapping does not allocate its full logical length, and
- writes at both ends of the mapping are visible through another view.
