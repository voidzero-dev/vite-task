# fetch_env_sees_command_prefix_env

A command-prefixed env (`PREFIXED_ENV=from-command node ...`) is part of the spawn's full env context. `getEnv('PREFIXED_ENV')` and `process.env.PREFIXED_ENV` must both see the command prefix value.

## `vt run fetch-prefixed-env`

tool asks the runner for an env the command prefix sets

```
$ PREFIXED_ENV=from-command node scripts/fetch_env.mjs PREFIXED_ENV
served PREFIXED_ENV=from-command
process.env PREFIXED_ENV=from-command
```
