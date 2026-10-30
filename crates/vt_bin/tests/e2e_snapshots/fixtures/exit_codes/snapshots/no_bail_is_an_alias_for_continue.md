# no_bail_is_an_alias_for_continue

`--no-bail`, pnpm's name for the same behavior, works like `--continue`. With a concurrency limit of 1, fail-3 and fail-5 run one after the other, so the second only runs if the first failure doesn't cancel the run.

## `vt run --no-bail --concurrency-limit 1 two-failures`

fail-3 and fail-5 both run

**Exit code:** 1

```
~/packages/pkg-c$ vtt exit 3

~/packages/pkg-c$ vtt exit 5

---
vt run: 0/2 cache hit (0%), 0/2 successful. (Run `vt run --last-details` for full details)
```
