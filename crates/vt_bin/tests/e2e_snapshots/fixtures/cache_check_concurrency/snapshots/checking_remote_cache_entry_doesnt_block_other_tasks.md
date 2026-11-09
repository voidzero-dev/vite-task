# checking_remote_cache_entry_doesnt_block_other_tasks

## `remote-cache-server start`

```
```

## `VP_REMOTE_CACHE=read-write VP_RUN_INTERNAL_HIDE_PENDING_UPLOADS=1 remote-cache-server run --github-actions vt run --ignore-depends-on read`

read reads input.txt, and its entry is uploaded.

```
$ vtt print-file input.txt
input

[remote-cache] POST /fetch 404
[remote-cache] POST /store 200
```

## `vt cache clean`

```
```

## `VP_REMOTE_CACHE=read remote-cache-server run vt run test`

Other tasks run while read's remote entry is checked, and then it hits.

```
$ vtt hold-reads input.txt .barrier ⊘ cache disabled
$ vtt barrier .barrier held 2 ⊘ cache disabled
$ vtt print other ⊘ cache disabled
other
$ vtt barrier .barrier released 2 ⊘ cache disabled
$ vtt print-file input.txt ◉ remote cache hit, replaying
input
$ vtt print done ⊘ cache disabled
done
---
vt run: 1/1 cache hit (100%, 1 remote), 6/6 successful. (Run `vt run --last-details` for full details)
[remote-cache] POST /fetch 200 exact [-]
```

## `remote-cache-server stop`

```
```
