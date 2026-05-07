# auto_output_with_non_auto_input

Auto output works even when input tracking is explicit
(`input: ["src/**"]`). Output auto-detection and input auto-detection
are independent.

## `vt run auto-output-no-auto-input`

first run: input src/** (no auto), output default (auto)

```
$ vtt write-file dist/out.txt built
```

## `vtt rm dist/out.txt`

delete the output

```
```

## `vt run auto-output-no-auto-input`

cache hit - output files should still be restored

```
$ vtt write-file dist/out.txt built ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vtt print-file dist/out.txt`

output was restored

```
built
```
