# fetch_env_missing_returns_undefined

Exercises the public `getEnv(name)` contract for an absent env var. The client API must expose `undefined` so callers can distinguish absence using the documented API.

## `vt run fetch-missing-env`

missing env values are normalized to undefined

```
$ node scripts/assert_undefined_env.mjs
missing undefined
```
