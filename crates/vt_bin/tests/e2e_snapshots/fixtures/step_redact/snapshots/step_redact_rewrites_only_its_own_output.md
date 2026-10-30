# step_redact_rewrites_only_its_own_output

A step's `redact` table rewrites regex matches in that step's output and leaves other steps' output alone.

## `vtt print value: 8f3a1c`

`redact` replaces the value

```
value: <value>
```

## `vtt print value: 8f3a1c`

no `redact`, so the value stays

```
value: 8f3a1c
```
