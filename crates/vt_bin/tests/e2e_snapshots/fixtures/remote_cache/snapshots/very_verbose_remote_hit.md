# very_verbose_remote_hit

With `-vv`, a remote hit's outputs are listed once its archive is downloaded. A dry run doesn't download it, so it says they aren't, and a real run lists them. A remote hit without outputs has none to list either way.

## `remote-cache-server start`

```
```

## `VP_REMOTE_CACHE=read-write VP_RUN_INTERNAL_HIDE_PENDING_UPLOADS=1 remote-cache-server run --github-actions vt run build`

```
$ vtt write-file dist/output.txt built

[remote-cache] POST /fetch 404
[remote-cache] POST /store 200
```

## `VP_REMOTE_CACHE=read-write VP_RUN_INTERNAL_HIDE_PENDING_UPLOADS=1 remote-cache-server run --github-actions vt run check`

```
$ vtt print checked
checked

[remote-cache] POST /fetch 404
[remote-cache] POST /store 200
```

## `vt cache clean`

```
```

## `remote-cache-server run vt run --dry-run -vv build`

A dry run doesn't download the archive, so the outputs aren't listed.

```
$ vtt write-file dist/output.txt built ◉ remote cache hit

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 task • 1 cache hit (1 remote) • 0 cache misses
Performance:  100% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] remote-cache#build: $ vtt write-file dist/output.txt built
      → Remote cache hit
        Unchanged: 'src/a.txt'
        Outputs:   (not downloaded)
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
[remote-cache] POST /fetch 200 exact [1]
```

## `remote-cache-server run vtt pipe-stdout -- vt run --dry-run --json -vv build`

In the JSON, they're `notDownloaded`. Hashes depend on file contents, so they're redacted.

```
--- stdout ---
{"$schema":"<schema>","version":1,"workspaceRoot":"<workspace>","plan":{"build":{"package":".","name":"build","steps":[{"program":"<vtt>","args":["write-file","dist/output.txt","built"],"cache":{"status":"hit","baseline":{"durationMs":<ms>,"fingerprint":{"inputs":{"src/a.txt":{"type":"file","hash":"<hash>"}}},"outputs":"notDownloaded"}}}]}}}
[remote-cache] POST /fetch 200 exact [1]
```

## `remote-cache-server run vtt pipe-stdout -- vt run --dry-run --json -vv check`

`check` has no outputs, which the entry says without its archive, so there's no `outputs`.

```
--- stdout ---
{"$schema":"<schema>","version":1,"workspaceRoot":"<workspace>","plan":{"check":{"package":".","name":"check","steps":[{"program":"<vtt>","args":["print","checked"],"cache":{"status":"hit","baseline":{"durationMs":<ms>,"fingerprint":{"inputs":{"src/a.txt":{"type":"file","hash":"<hash>"}}}}}}]}}}
[remote-cache] POST /fetch 200 exact [-]
```

## `remote-cache-server run vt run -vv build`

A real run downloads the archive, so it lists the outputs it restores.

```
$ vtt write-file dist/output.txt built ◉ remote cache hit, replaying


━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 task • 1 cache hit (1 remote) • 0 cache misses
Performance:  100% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] remote-cache#build: $ vtt write-file dist/output.txt built ✓
      → Remote cache hit - output replayed -
        Unchanged: 'src/a.txt'
        Outputs:   'dist/output.txt'
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
[remote-cache] POST /fetch 200 exact [1]
[remote-cache] GET /blob/1 200
```

## `remote-cache-server stop`

```
```
