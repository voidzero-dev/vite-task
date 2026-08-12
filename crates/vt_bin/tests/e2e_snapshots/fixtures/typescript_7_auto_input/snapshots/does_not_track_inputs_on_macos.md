# does_not_track_inputs_on_macos

TypeScript 7's native compiler bypasses the library calls fspy interposes when running in Codex's workspace sandbox on macOS. This documents the current limitation: changing a source file is not detected, so the cached typecheck is replayed.

## `tsc --version`

```
Version 7.0.2
```

## `codex sandbox -P :workspace vt run typecheck`

```
$ tsc --noEmit
```

## `vtt replace-file-content src/main.ts initial modified`

```
```

## `codex sandbox -P :workspace vt run typecheck`

```
$ tsc --noEmit ◉ cache hit, replaying

---
vt run: cache hit.
```
