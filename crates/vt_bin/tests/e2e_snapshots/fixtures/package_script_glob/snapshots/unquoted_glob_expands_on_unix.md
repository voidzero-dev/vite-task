# unquoted_glob_expands_on_unix

Unquoted pathname patterns in package scripts are executed by the platform shell, matching package-manager script semantics on Unix.

## `vt run unquoted-glob`

```
$ vtt print packages/*/src ⊘ cache disabled
packages/a/src packages/b/src
```
