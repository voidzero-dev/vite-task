# malformed_observed_path_does_not_panic

Repro for issue 325: a malformed observed path must not panic when fspy
input inference normalizes workspace-relative accesses.

## `TEMP=. TMP=. vt run read-malformed-path`

```
$ vtt print-file foo/C:/bar
foo/C:/bar: not found
```
