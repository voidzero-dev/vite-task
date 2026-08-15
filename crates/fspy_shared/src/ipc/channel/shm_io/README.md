# shm_io: the fspy frame channel

One shared-memory region. Many writer processes append records; one
receiver reads them once, at the end of a run. The writers are traced build
processes reporting file accesses; the receiver is the runner deciding
whether the run can be cached.

Three requirements shaped everything here:

1. **A writer may die at any instruction** — SIGKILL included. This must
   never corrupt the channel or lose another writer's records.
2. **A writer may outlive the run** (a daemon). The receiver must never
   wait for writers; closing is immediate.
3. **The result must be safe to cache from.** Either the trace holds every
   record the run reported, or the run is declared uncacheable. Never a
   silently short trace.

Earlier designs failed these. A file lock proved "no writers left", but a
process that closes file descriptors it does not recognize releases the
lock while its memory mapping keeps writing — the reader then parsed
half-written bytes (issue #544). Waiting for writers hangs forever on
daemons. Counting active writers breaks because a killed process never
decrements the count.

## The region

The channel is a sparse file in the temp directory, mapped into every
participating process. It is address space, not memory: only pages that
are actually written get backed.

```text
| header (64 B) | descriptor table (1/8 of the region) | payloads (the rest, grow up) |
```

The header holds three words, and every one of them only ever counts up:

- the **claim counter** — how many frames were ever claimed. Bit 63 is the
  CLOSED gate.
- the **incomplete flag** — nonzero once any live writer lost a record.
- the **payload counter** — how many payload bytes were ever reserved,
  including by failed claims. Only writers read it.

The table has one 8-byte slot per frame. For the production 4 GiB region
that is ~67 million slots; the ~3.5 GiB payload region fits ~15–20 million
typical records, so payload space runs out first. The split is fixed, not a
movable frontier, because fixed bounds are what make claiming wait-free:
each counter is checked against its own constant limit using the value
`fetch_add` returned, and overshooting a limit is harmless because nothing
ever locates data through a counter — descriptors are self-describing.

## Writing a frame

Three steps:

1. **Claim.** Two `fetch_add`s — one reserves payload bytes, one reserves a
   slot. No retry loop, no lock. A claim that does not fit fails after the
   fact; the wasted counter space does not matter (see above).
2. **Fill.** The writer serializes into its payload span. The span is
   exclusively its own; nobody else knows it exists yet.
3. **Commit.** One compare-and-swap flips the frame's slot from zero to a
   descriptor holding the payload's offset and length. The CAS is the
   publication point: before it, the frame does not exist; after it, the
   payload is immutable.

Committing is explicit (`FrameMut::finish`). What happens when it never
runs is the heart of the design:

- **The process died** — mid-claim, mid-fill, anywhere. The slot stays
  zero. The receiver ignores it. Nothing else is affected, and no cleanup
  code ever runs or is needed.
- **The process is alive but abandoned the frame** (dropped it without
  finishing). The drop sets the incomplete flag, because the process will
  go on to perform the file operation it just failed to record — the trace
  now under-reports, so the run must not be cached.

The rule that makes ignoring dead writers safe: **a record is committed
before the recorded operation is performed.** A dead writer's missing
record is an operation that never happened. A record refused after close
belongs to an operation performed after the run's boundary.

## Closing and reading

The receiver closes once, at the end of the run:

1. **Snapshot** the claim counter with a plain load. This is the boundary:
   claims at or before it are in the run, later ones are not.
2. **Gate** further claims by setting the CLOSED bit, so stragglers stop.
3. **Freeze** every slot in the snapshot: a compare-and-swap flips zero to
   ABORTED. If the slot was already committed, the swap fails and the frame
   is kept. Exactly one side wins each slot; both outcomes are terminal.
4. **Validate** each committed descriptor's bounds. A descriptor no correct
   writer could produce fails the whole trace — never a panic, never an
   out-of-bounds read.

The result, `Frames`, owns the mapping and lends out one `&[u8]` per
committed span, straight from shared memory — no copy. The borrows are
sound because a committed span is never written again and is disjoint from
everything a live straggler may still touch. The mapping is released when
`Frames` is dropped.

```text
                         writer's commit CAS wins
                    +------------------------------> COMMITTED (readable)
CLAIMED (slot 0) ---+
                    +------------------------------> ABORTED (ignored)
                         receiver's freeze CAS wins
```

## Why this is sound, in one list

- Frame traversal never reads payload bytes; every slot has a fixed place.
  The #544 failure class (payload parsed as metadata) is structurally gone.
- A payload is reachable only through its committed descriptor. The commit
  is a `Release` write and the receiver's failed freeze is an `Acquire`
  read, so an observed descriptor implies fully visible payload bytes.
- Committed and aborted are terminal. No code path changes a terminal slot.
- Counters only grow. The receiver clamps them to the fixed capacities, so
  an inflated counter degrades into extra aborted slots, not corruption.
- The bounds checks on descriptors are what justify the `unsafe` borrow
  construction: the receiver's memory safety never depends on another
  process being correct.
- Byte _integrity_ does trust protocol compliance — a process scribbling
  random memory is outside the model. That trust buys the borrow-in-place
  reader and the absence of checksums.

## Performance notes

- Claiming is two atomic adds; committing is one CAS. Nothing retries.
- Closing costs one pass over the claimed slots. No payload is copied.
- On Linux, the first touch of the sparse file can cost milliseconds on
  journalling filesystems (it is the fault path, not block allocation —
  `fallocate` does not help). Channel creation therefore pre-touches the
  header page on a background thread, concurrently with process startup.
  Windows and macOS fault cheaply and skip this.

## Files

| File        | Role                                                                                                                                                                                                                |
| ----------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `mod.rs`    | Public surface (`ShmWriter`, `FrameMut`, `close`, `Frames`), the protocol overview docs, and the integration tests — they run against a mocked region and are miri-clean (`cargo miri test -p fspy_shared shm_io`). |
| `layout.rs` | Pure geometry: header offsets, the table/payload split, span validation. Plain integer math, no pointers, no atomics.                                                                                               |
| `slot.rs`   | The descriptor codec: pack and unpack one slot value, classify it as unfinished, aborted, committed, or corrupt. Pure.                                                                                              |
| `state.rs`  | The only module that touches shared memory. All atomics, every unsafe pointer derivation, and the three-rule memory-ordering contract live here.                                                                    |
| `writer.rs` | Claim, fill, finish. Owns the argument for why a claimed payload span is exclusively the writer's.                                                                                                                  |
| `reader.rs` | Close (snapshot, gate, freeze, validate) and `Frames`. Owns the argument for why borrowing committed spans is sound.                                                                                                |

Each file carries one self-contained argument, so the protocol can be
reviewed module by module: the pure math first, then the atomics, then the
two aliasing arguments built on top of them.
