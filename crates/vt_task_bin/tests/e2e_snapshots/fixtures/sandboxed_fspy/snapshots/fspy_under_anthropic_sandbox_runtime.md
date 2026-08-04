# fspy_under_anthropic_sandbox_runtime

Runs `vt run inner` under a profile matching Claude Code's default enabled Bash sandbox: the working directory and session temp directory are writable, with no pre-allowed network domains or extra Unix socket access. See [Claude Code sandboxing](https://code.claude.com/docs/en/sandboxing#filesystem-isolation).

Claude Code normally creates the writable session temp before invoking Sandbox Runtime, so the harness creates the runtime's default `/tmp/claude` directory before calling `srt` directly. The fixture profile provides only `allowWrite: ["."]`; Sandbox Runtime supplies `/tmp/claude` and its other built-in compatibility paths. See [`getDefaultWritePaths` and `generateProxyEnvVars`](https://github.com/anthropic-experimental/sandbox-runtime/blob/main/src/sandbox/sandbox-utils.ts).

The nested `vt` enables fspy for automatic input inference; changing the file read inside the sandbox checks whether it invalidates the cache.

## `mkdir -p /tmp/claude`

create the session temp that Claude Code supplies before sandboxed Bash commands

```
```

## `srt --settings claude-code-default-sandbox.json vt run inner`

**Exit code:** 1

```
$ vtt print-file input.txt
✗ Failed to set up task communication: Operation not permitted (os error 1)
```

## `vtt replace-file-content input.txt tracked modified`

```
```

## `srt --settings claude-code-default-sandbox.json vt run inner`

**Exit code:** 1

```
$ vtt print-file input.txt
✗ Failed to set up task communication: Operation not permitted (os error 1)
```
