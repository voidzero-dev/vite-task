# macOS backend

The macOS backend uses named POSIX shared memory. It provides string-based
discovery, demand-backed mapped storage, and POSIX unlink semantics without a
filesystem path or descriptor broker.

## Requirements

The backend must provide:

- a serialized identifier that another process can open,
- demand-backed storage for a large logical mapping,
- memory and atomic operations for concurrent writers,
- exact logical-size validation, and
- owner cleanup that preserves already-open views while preventing later
  opens.

## Options considered

| Option                               | Decision                                                                                                            |
| ------------------------------------ | ------------------------------------------------------------------------------------------------------------------- |
| Anonymous `mmap` with inheritance    | Rejected. Processes created later cannot recover the mapping from a string identifier.                              |
| Temporary file mapping               | Rejected. A pathname, cleanup protocol, and possible disk writeback add no benefit over native POSIX shared memory. |
| Mach memory entry with port transfer | Rejected. It would require a Mach-port broker to turn the entry into a serialized identifier.                       |
| POSIX `shm_open`                     | Selected. It directly supplies naming, cross-process opening, demand-backed mapping, and unlink semantics.          |

Linux rejects POSIX `shm_open` because Linux normally routes it through the
container's size-limited `/dev/shm` mount. macOS does not have that mount-level
constraint, so a broker would add complexity without solving a macOS problem.

## Why not `shared_memory`

The `shared_memory` macOS backend uses the same POSIX primitive, but its open
path ignores the caller's expected size and reports `fstat().st_size` as the
mapping length. Darwin can report that size rounded to a virtual-memory page,
so the facade can expose more bytes than the logical channel capacity and
cannot reject nearby size mismatches.

The local backend maps the requested logical size. It validates the reported
object size and a compact size tag in the identifier, which distinguishes
logical sizes that Darwin rounds to the same page.

Owning this small backend also removes `shared_memory`'s persistence,
ownership-transfer, logging, and compatibility surface. Fspy needs only named
creation, opening, mapping, exact-size validation, and owner unlink.

## Lifetime semantics

Dropping the owner unlinks the POSIX name, preventing later opens. Existing
views remain valid until each process unmaps its own view. Initialization
failures after name creation also unlink the name rather than leaving a stale
shared-memory object.
