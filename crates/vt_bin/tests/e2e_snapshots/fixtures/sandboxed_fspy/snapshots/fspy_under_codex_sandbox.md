# fspy_under_codex_sandbox

Runs `vt run inner` under Codex CLI's built-in `:workspace` permission profile, representing its default workspace-editing posture for a trusted repository. The profile makes the workspace roots and system temp directories writable without adding network or Unix socket access. See [Codex permissions](https://learn.chatgpt.com/docs/permissions#define-and-select-a-profile).

The nested `vt` enables fspy for automatic input inference; changing the file read inside the sandbox checks whether it invalidates the cache.

## `codex sandbox -P :workspace vt run inner`

```
$ vtt print-file input.txt
tracked input
```

## `vtt replace-file-content input.txt tracked modified`

```
```

## `codex sandbox -P :workspace vt run inner`

```
$ vtt print-file input.txt ○ cache miss: 'input.txt' modified, executing
modified input
```
