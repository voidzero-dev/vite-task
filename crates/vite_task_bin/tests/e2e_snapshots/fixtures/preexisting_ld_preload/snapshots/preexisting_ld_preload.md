# preexisting_ld_preload

Reproduces #340 and verifies that fspy tolerates a user-supplied
`LD_PRELOAD` by appending its shim instead of rejecting the spawn.
Appending (not prepending) also preserves symbol-interposition order:
the user's preload runs first, so short-circuited calls remain
invisible to fspy — what the OS actually executed is what fspy records.

`preload_test_lib` (built as a cdylib artifact dep) intercepts
`open`/`openat` and short-circuits any path containing the marker
`preload_test_short_circuit`, returning `ENOENT` without forwarding.
Every other path is forwarded via `RTLD_NEXT` so fspy still observes
the real syscall.

The `read` task prints two files. `real.txt` goes through the full
interposer chain and is tracked as an input. `preload_test_short_circuit.txt`
is short-circuited by the user preload; fspy never sees it and does not
track it. Modifying the short-circuited file must therefore be a cache
hit; modifying the real file must be a miss.

## `LD_PRELOAD=<PRELOAD_TEST_LIB_PATH> vt run read`

cache miss: real.txt tracked; short-circuited file reported not found

```
$ vtt print-file real.txt
real content

$ vtt print-file preload_test_short_circuit.txt
preload_test_short_circuit.txt: not found

---
vt run: 0/2 cache hit (0%). (Run `vt run --last-details` for full details)
```

## `LD_PRELOAD=<PRELOAD_TEST_LIB_PATH> vt run read`

cache hit

```
$ vtt print-file real.txt ◉ cache hit, replaying
real content

$ vtt print-file preload_test_short_circuit.txt ◉ cache hit, replaying
preload_test_short_circuit.txt: not found

---
vt run: 2/2 cache hit (100%). (Run `vt run --last-details` for full details)
```

## `vtt write-file preload_test_short_circuit.txt 'modified short-circuited content'`

modify the untracked (short-circuited) file

```
```

## `LD_PRELOAD=<PRELOAD_TEST_LIB_PATH> vt run read`

still cache hit: short-circuited access was never tracked

```
$ vtt print-file real.txt ◉ cache hit, replaying
real content

$ vtt print-file preload_test_short_circuit.txt ◉ cache hit, replaying
preload_test_short_circuit.txt: not found

---
vt run: 2/2 cache hit (100%). (Run `vt run --last-details` for full details)
```

## `vtt write-file real.txt 'modified real content'`

modify the tracked file

```
```

## `LD_PRELOAD=<PRELOAD_TEST_LIB_PATH> vt run read`

cache miss: tracked input changed

```
$ vtt print-file real.txt ○ cache miss: 'real.txt' modified, executing
modified real content
$ vtt print-file preload_test_short_circuit.txt ◉ cache hit, replaying
preload_test_short_circuit.txt: not found

---
vt run: 1/2 cache hit (50%). (Run `vt run --last-details` for full details)
```
