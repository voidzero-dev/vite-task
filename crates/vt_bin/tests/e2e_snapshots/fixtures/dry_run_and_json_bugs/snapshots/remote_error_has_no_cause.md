# remote_error_has_no_cause

Bug: when the remote cache can't be read, the text with `-v` says why, but the JSON's `message` never does, with `-v` or without.

## `VP_REMOTE_CACHE_URL=http://127.0.0.1:0/projects/test vt run --dry-run -v build`

Nothing can listen on port 0. The detailed summary lists the causes.

```
$ vtt print-file src/a.txt ○ cache miss: remote cache fetch failed

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 task • 0 cache hits • 1 cache miss
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] dry-run-and-json-bugs#build: $ vtt print-file src/a.txt
      → Cache miss: remote cache fetch failed
        ↳ network error
        ↳ error sending request
        ↳ client error (Connect)
        ↳ tcp connect error
        ↳ <os error>
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## `VP_REMOTE_CACHE_URL=http://127.0.0.1:0/projects/test vtt pipe-stdout -- vt run --dry-run --json -v build`

Expected: the causes, as in the text. The message is only `remote cache fetch failed`.

```
--- stdout ---
{"$schema":"<schema>","version":1,"workspaceRoot":"<workspace>","plan":{"build":{"package":".","name":"build","steps":[{"program":"<vtt>","args":["print-file","src/a.txt"],"cache":{"status":"error","message":"remote cache fetch failed"}}]}}}
```
