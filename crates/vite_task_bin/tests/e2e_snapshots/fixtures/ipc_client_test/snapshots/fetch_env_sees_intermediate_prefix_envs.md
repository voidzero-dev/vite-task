# fetch_env_sees_intermediate_prefix_envs

Prefix envs accumulate through nested runs: `outer-prefixed` is `PREFIXED_A=a vt run inner-prefixed`, and `inner-prefixed` is `PREFIXED_B=b node ...`. The inner tool's `getEnv` must see both through the runner's env context, while `process.env` only sees the direct child prefix.

## `vt run outer-prefixed`

outer prefix env reaches the inner tool via the runner

```
$ PREFIXED_B=b node scripts/fetch_env.mjs PREFIXED_A PREFIXED_B
served PREFIXED_A=a PREFIXED_B=b
process.env PREFIXED_A=(unset) PREFIXED_B=b
```
