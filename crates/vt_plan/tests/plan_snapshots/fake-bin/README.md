# fake-bin

Stub executables used by plan snapshot tests.

Plan tests need `vtt` to be resolvable on `PATH` so that `find_executable` can produce
an absolute path for task commands like `vtt print-file package.json`. The binary is
never actually executed during plan tests — only its resolved path appears in snapshots.

- `vtt` — Unix stub (executable shell script)
- `vtt.cmd` — Windows stub (looked up via `PATHEXT`)
- `vpr`, `vpr.cmd` — the same stubs for `vpr`, which is spawned as written when a script `cd`s into a project outside the workspace
