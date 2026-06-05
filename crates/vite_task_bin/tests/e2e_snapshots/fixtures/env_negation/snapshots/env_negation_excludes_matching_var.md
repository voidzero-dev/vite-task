# env_negation_excludes_matching_var

A `!`-prefixed env pattern excludes matching variables. With `env: ["PROBE_*", "!PROBE_SECRET"]`, the task receives PROBE_PUBLIC but not PROBE_SECRET — the negation filters it out — so print-env reports it undefined.

## `PROBE_PUBLIC=public-value PROBE_SECRET=secret-value vt run print`

PROBE_SECRET is filtered out by !PROBE_SECRET

```
$ vtt print-env PROBE_PUBLIC
public-value

$ vtt print-env PROBE_SECRET
(undefined)

---
vt run: 0/2 cache hit (0%). (Run `vt run --last-details` for full details)
```
