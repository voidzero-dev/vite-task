# fetch_envs_untracked_does_not_invalidate

Exercises `getEnvs(pattern, { tracked: false })`. The runner still serves the matching envs, but the match set does not enter the post-run fingerprint, so changing it later replays the cached output.

## `PROBE_A=a PROBE_B=b vt run fetch-envs-untracked`

first run serves the PROBE_* match set without tracking it

```
$ node scripts/fetch_envs.mjs --untracked
PROBE_A=a
PROBE_B=b
```

## `PROBE_A=changed PROBE_B=b PROBE_C=c vt run fetch-envs-untracked`

cache hit: changed match set was requested with tracked: false

```
$ node scripts/fetch_envs.mjs --untracked ◉ cache hit, replaying
PROBE_A=a
PROBE_B=b

---
vt run: cache hit.
```
