# unknown_after_remote_hit

As after a local hit, the steps after a remote hit are `unknown` only if it restores outputs. The fetched entry says whether it does, so a dry run knows without downloading the archive: there's no `GET /blob` request.

## `remote-cache-server start`

```
```

## `VP_REMOTE_CACHE=read-write VP_RUN_INTERNAL_HIDE_PENDING_UPLOADS=1 remote-cache-server run --github-actions vt run emit`

`emit` runs first, on its own, so its upload doesn't overlap `after-emit`'s fetch.

```
$ vtt write-file dist/out.txt built

[remote-cache] POST /fetch 404
[remote-cache] POST /store 200
```

## `VP_REMOTE_CACHE=read-write VP_RUN_INTERNAL_HIDE_PENDING_UPLOADS=1 remote-cache-server run --github-actions vt run after-emit`

```
$ vtt write-file dist/out.txt built ◉ cache hit, replaying

$ vtt print checked
checked

---
vt run: 1/2 cache hit (50%), 2/2 successful. (Run `vt run --last-details` for full details)
[remote-cache] POST /fetch 404
[remote-cache] POST /store 200
```

## `VP_REMOTE_CACHE=read-write VP_RUN_INTERNAL_HIDE_PENDING_UPLOADS=1 remote-cache-server run --github-actions vt run build`

`build` runs first, on its own, so its upload doesn't overlap `after-build`'s fetch.

```
$ vtt print-file a.txt b.txt
a
b

[remote-cache] POST /fetch 404
[remote-cache] POST /store 200
```

## `VP_REMOTE_CACHE=read-write VP_RUN_INTERNAL_HIDE_PENDING_UPLOADS=1 remote-cache-server run --github-actions vt run after-build`

`after-build` runs the same command as `after-emit`, so that step hits the entry `after-emit`'s run stored, and nothing is fetched.

```
$ vtt print-file a.txt b.txt ◉ cache hit, replaying
a
b

$ vtt print checked ◉ cache hit, replaying
checked

---
vt run: 2/2 cache hit (100%), 2/2 successful. (Run `vt run --last-details` for full details)
```

## `vt cache clean`

```
```

## `remote-cache-server run vtt pipe-stdout -- vt run --dry-run --json after-emit`

`emit` is a remote hit that restores `dist/out.txt`, so `after-emit` is unknown.

```
--- stdout ---
{"$schema":"<schema>","version":1,"workspaceRoot":"<workspace>","plan":{"after-emit":{"package":".","name":"after-emit","dependsOn":["emit"],"steps":[{"program":"<vtt>","args":["print","checked"],"cache":{"status":"unknown","baseline":{"durationMs":<ms>}}}]},"emit":{"package":".","name":"emit","steps":[{"program":"<vtt>","args":["write-file","dist/out.txt","built"],"cache":{"status":"hit","baseline":{"durationMs":<ms>}}}]}}}
[remote-cache] POST /fetch 200 exact [1]
[remote-cache] POST /fetch 200 exact [-]
```

## `remote-cache-server run vtt pipe-stdout -- vt run --dry-run --json after-build`

`build` is a remote hit without outputs, so `after-build` stays a hit.

```
--- stdout ---
{"$schema":"<schema>","version":1,"workspaceRoot":"<workspace>","plan":{"after-build":{"package":".","name":"after-build","dependsOn":["build"],"steps":[{"program":"<vtt>","args":["print","checked"],"cache":{"status":"hit","baseline":{"durationMs":<ms>}}}]},"build":{"package":".","name":"build","steps":[{"program":"<vtt>","args":["print-file","a.txt","b.txt"],"cache":{"status":"hit","baseline":{"durationMs":<ms>}}}]}}}
[remote-cache] POST /fetch 200 exact [-]
[remote-cache] POST /fetch 200 exact [-]
```

## `remote-cache-server stop`

```
```
