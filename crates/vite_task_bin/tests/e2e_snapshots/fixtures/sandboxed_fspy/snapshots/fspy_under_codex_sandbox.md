# fspy_under_codex_sandbox

Runs `vt run inner` under Codex CLI's built-in `:workspace` permission profile, representing its default workspace-editing posture for a trusted repository. The profile makes the workspace roots and system temp directories writable without adding network or Unix socket access. See [Codex permissions](https://learn.chatgpt.com/docs/permissions#define-and-select-a-profile).

The nested `vt` enables fspy for automatic input inference; changing the file read inside the sandbox checks whether it invalidates the cache.

## `codex sandbox -P :workspace vt run inner`

**Exit code:** 1

```
$ vtt print-file input.txt
✗ Failed to spawn process: failed to create IPC channel: Operation not permitted (os error 1)
```

## `vtt replace-file-content input.txt tracked modified`

```
```

## `codex sandbox -P :workspace vt run inner`

**Exit code:** 1

```
$ vtt print-file input.txt
✗ Failed to spawn process: failed to create IPC channel: Operation not permitted (os error 1)
```
