# worktree_replays_checkout_entry

A task that lists the workspace root sees a `.git` entry, which is a directory in a checkout and a file in a `git worktree`. The entry is not part of the fingerprint, so a worktree replays the entry a checkout recorded.

## `vtt mkdir .git`

a checkout: `.git` is a directory

```
```

## `vt run list-root`

cache miss

```
$ vtt list-dir . --ext .json
package.json
vite-task.json
```

## `vtt rm -r .git`

remove the checkout's `.git` directory

```
```

## `vtt write-file .git 'gitdir: ../checkout/.git/worktrees/feature'`

a worktree: `.git` is a file

```
```

## `vt run list-root`

cache hit

```
$ vtt list-dir . --ext .json ◉ cache hit, replaying
package.json
vite-task.json

---
vt run: cache hit.
```
