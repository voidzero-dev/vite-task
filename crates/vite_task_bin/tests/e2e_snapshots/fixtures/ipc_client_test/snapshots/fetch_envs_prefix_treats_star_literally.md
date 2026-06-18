# fetch_envs_prefix_treats_star_literally

Exercises `getEnvs({ prefix: "PROBE_*" })`: `*` is part of the prefix string, not a glob wildcard.

## `PROBE_*A=literal PROBE_XA=wildcard-if-glob PROBE_A=also-wildcard-if-glob vt run fetch-envs-star-prefix`

runner serves only envs whose name starts with literal PROBE_*

```
$ node scripts/fetch_envs.mjs --prefix PROBE_*
PROBE_*A=literal
```
