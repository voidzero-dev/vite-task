# lists_what_cached_runs_depend_on

With `-vv`, the detailed summary also lists what each cached run depends on that didn't change, and the files it restores. The steps list a folder, read a file, look for a missing one, and write an output.

## `vt run vv`

```
$ vtt list-dir src
a.txt

$ vtt print-file src/a.txt
a

$ vtt stat-file missing.txt
missing.txt: missing

$ vtt write-file dist/out.txt built

---
vt run: 0/4 cache hit (0%), 4/4 successful. (Run `vt run --last-details` for full details)
```

## `vt run --dry-run -vv vv`

Every step hits.

```
$ vtt list-dir src ◉ cache hit
$ vtt print-file src/a.txt ◉ cache hit
$ vtt stat-file missing.txt ◉ cache hit
$ vtt write-file dist/out.txt built ◉ cache hit

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   4 tasks • 4 cache hits • 0 cache misses
Performance:  100% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] very-verbose#vv: $ vtt list-dir src
      → Cache hit
        Unchanged: 'src' (listing)
  ·······················································
  [2] very-verbose#vv: $ vtt print-file src/a.txt
      → Cache hit
        Unchanged: 'src/a.txt'
  ·······················································
  [3] very-verbose#vv: $ vtt stat-file missing.txt
      → Cache hit
        Unchanged: 'missing.txt' (missing)
  ·······················································
  [4] very-verbose#vv: $ vtt write-file dist/out.txt built
      → Cache hit
        Outputs:   'dist/out.txt'
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## `vtt write-file src/b.txt new`

```
```

## `vt run --dry-run -vv vv`

The listed folder changed, so it isn't among what's unchanged.

```
$ vtt list-dir src ○ cache miss: 'b.txt' added in 'src'
$ vtt print-file src/a.txt ◉ cache hit
$ vtt stat-file missing.txt ◉ cache hit
$ vtt write-file dist/out.txt built ◉ cache hit

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   4 tasks • 3 cache hits • 1 cache miss
Performance:  75% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] very-verbose#vv: $ vtt list-dir src
      → Cache miss: 'b.txt' added in 'src'
  ·······················································
  [2] very-verbose#vv: $ vtt print-file src/a.txt
      → Cache hit
        Unchanged: 'src/a.txt'
  ·······················································
  [3] very-verbose#vv: $ vtt stat-file missing.txt
      → Cache hit
        Unchanged: 'missing.txt' (missing)
  ·······················································
  [4] very-verbose#vv: $ vtt write-file dist/out.txt built
      → Cache hit
        Outputs:   'dist/out.txt'
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## `vtt pipe-stdout -- vt run --dry-run --json -vv vv`

The JSON has each baseline's fingerprint and outputs. Hashes depend on file contents, so they're redacted.

```
--- stdout ---
{"$schema":"<schema>","version":1,"workspaceRoot":"<workspace>","plan":{"vv":{"package":".","name":"vv","steps":[{"program":"<vtt>","args":["list-dir","src"],"cache":{"status":"changed","baseline":{"durationMs":<ms>,"fingerprint":{"paths":{"src":{"type":"dir","entries":["a.txt"]}}}},"changes":[{"kind":"file","path":"src/b.txt","change":"added"}]}},{"program":"<vtt>","args":["print-file","src/a.txt"],"cache":{"status":"unknown","baseline":{"durationMs":<ms>,"fingerprint":{"paths":{"src/a.txt":{"type":"file","hash":"<hash>"}}}}}},{"program":"<vtt>","args":["stat-file","missing.txt"],"cache":{"status":"unknown","baseline":{"durationMs":<ms>,"fingerprint":{"paths":{"missing.txt":{"type":"missing"}}}}}},{"program":"<vtt>","args":["write-file","dist/out.txt","built"],"cache":{"status":"unknown","baseline":{"durationMs":<ms>,"outputs":["dist/out.txt"]}}}]}}}
```

## `vt run -vv vv`

A real run lists the same.

```
$ vtt list-dir src ○ cache miss: 'b.txt' added in 'src', executing
a.txt
b.txt

$ vtt print-file src/a.txt ◉ cache hit, replaying
a

$ vtt stat-file missing.txt ◉ cache hit, replaying
missing.txt: missing

$ vtt write-file dist/out.txt built ◉ cache hit, replaying


━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   4 tasks • 3 cache hits • 1 cache miss
Performance:  75% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] very-verbose#vv: $ vtt list-dir src ✓
      → Cache miss: 'b.txt' added in 'src'
  ·······················································
  [2] very-verbose#vv: $ vtt print-file src/a.txt ✓
      → Cache hit - output replayed -
        Unchanged: 'src/a.txt'
  ·······················································
  [3] very-verbose#vv: $ vtt stat-file missing.txt ✓
      → Cache hit - output replayed -
        Unchanged: 'missing.txt' (missing)
  ·······················································
  [4] very-verbose#vv: $ vtt write-file dist/out.txt built ✓
      → Cache hit - output replayed -
        Outputs:   'dist/out.txt'
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## `vt run --last-details`

So does the saved summary.

```

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   4 tasks • 3 cache hits • 1 cache miss
Performance:  75% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] very-verbose#vv: $ vtt list-dir src ✓
      → Cache miss: 'b.txt' added in 'src'
  ·······················································
  [2] very-verbose#vv: $ vtt print-file src/a.txt ✓
      → Cache hit - output replayed -
        Unchanged: 'src/a.txt'
  ·······················································
  [3] very-verbose#vv: $ vtt stat-file missing.txt ✓
      → Cache hit - output replayed -
        Unchanged: 'missing.txt' (missing)
  ·······················································
  [4] very-verbose#vv: $ vtt write-file dist/out.txt built ✓
      → Cache hit - output replayed -
        Outputs:   'dist/out.txt'
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```
