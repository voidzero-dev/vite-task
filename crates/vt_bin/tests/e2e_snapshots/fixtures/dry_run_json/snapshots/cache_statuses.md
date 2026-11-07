# cache_statuses

Each step's `cache` shows what a dry run found. Durations vary, and `vtt` is outside the workspace, at an absolute path, so both are redacted.

## `vt run --dry-run --json build`

Nothing cached yet. In a terminal, the document is indented.

```
{
  "$schema": "<schema>",
  "version": 1,
  "workspaceRoot": "<workspace>",
  "plan": {
    "build": {
      "package": ".",
      "name": "build",
      "steps": [
        {
          "program": "<vtt>",
          "args": [
            "print-file",
            "a.txt",
            "b.txt"
          ],
          "cache": {
            "status": "noBaseline"
          }
        }
      ]
    }
  }
}
```

## `vt run build`

```
$ vtt print-file a.txt b.txt
a
b
```

## `vtt pipe-stdout -- vt run --dry-run --json build`

A hit. Captured, the document is on one line.

```
--- stdout ---
{"$schema":"<schema>","version":1,"workspaceRoot":"<workspace>","plan":{"build":{"package":".","name":"build","steps":[{"program":"<vtt>","args":["print-file","a.txt","b.txt"],"cache":{"status":"hit","baseline":{"durationMs":<ms>}}}]}}}
```

## `vtt write-file a.txt changed`

```
```

## `vtt pipe-stdout -- vt run --dry-run --json build`

One file changed. Without `-v`, the file check stops there, so the list may be partial.

```
--- stdout ---
{"$schema":"<schema>","version":1,"workspaceRoot":"<workspace>","plan":{"build":{"package":".","name":"build","steps":[{"program":"<vtt>","args":["print-file","a.txt","b.txt"],"cache":{"status":"changed","baseline":{"durationMs":<ms>},"changes":[{"kind":"file","path":"a.txt","change":"changed"}],"changesPartial":true}}]}}}
```

## `vtt write-file b.txt changed`

```
```

## `vtt pipe-stdout -- vt run --dry-run --json -v build`

With `-v`, every changed file is listed, by path, and the list is complete.

```
--- stdout ---
{"$schema":"<schema>","version":1,"workspaceRoot":"<workspace>","plan":{"build":{"package":".","name":"build","steps":[{"program":"<vtt>","args":["print-file","a.txt","b.txt"],"cache":{"status":"changed","baseline":{"durationMs":<ms>},"changes":[{"kind":"file","path":"a.txt","change":"changed"},{"kind":"file","path":"b.txt","change":"changed"}]}}]}}}
```

## `vtt pipe-stdout -- vt run --dry-run --json uncached`

Caching is off for this task.

```
--- stdout ---
{"$schema":"<schema>","version":1,"workspaceRoot":"<workspace>","plan":{"uncached":{"package":".","name":"uncached","steps":[{"program":"<vtt>","args":["print","uncached"],"cache":{"status":"disabled"}}]}}}
```

## `VP_REMOTE_CACHE_URL=http://127.0.0.1:0/projects/test vtt pipe-stdout -- vt run --dry-run --json other`

Nothing is cached locally, and the remote cache can't be read: nothing can listen on port 0.

```
--- stdout ---
{"$schema":"<schema>","version":1,"workspaceRoot":"<workspace>","plan":{"other":{"package":".","name":"other","steps":[{"program":"<vtt>","args":["print","other"],"cache":{"status":"error","message":"remote cache fetch failed"}}]}}}
```

## `vtt pipe-stdout -- vt run --dry-run --json --filter missing build`

No package matched, so the plan is empty.

```
No packages matched the filter: missing
--- stdout ---
{"$schema":"<schema>","version":1,"workspaceRoot":"<workspace>","plan":{}}
```
