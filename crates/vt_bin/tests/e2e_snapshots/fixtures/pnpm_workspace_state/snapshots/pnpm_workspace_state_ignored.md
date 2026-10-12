# pnpm_workspace_state_ignored

A task that runs `pnpm run` reads pnpm's `node_modules/.pnpm-workspace-state-v1.json`, which pnpm rewrites with a new timestamp whenever it rechecks dependencies. The file is not part of the fingerprint, so the task keeps its cache hit after another pnpm command rechecks.

## `pnpm install --frozen-lockfile --silent`

install, recording pnpm's dependency check

```
```

## `vtt cp node_modules/.pnpm-workspace-state-v1.json state-after-install.json`

save the state file

```
```

## `vt run pnpm-greet`

cache miss

```
$ pnpm --silent run greet
hello
```

## `vt run pnpm-greet`

cache hit

```
$ pnpm --silent run greet ◉ cache hit, replaying
hello

---
vt run: cache hit.
```

## `vtt replace-file-content package.json 'vtt print hello' 'vtt print goodbye'`

switch branches

```
```

## `vtt replace-file-content package.json 'vtt print goodbye' 'vtt print hello'`

switch back: package.json is unchanged but newer than pnpm's last check

```
```

## `pnpm --silent run greet`

pnpm rechecks dependencies and rewrites the state file

```
hello
```

## `node compare-state.mjs`

the state file changed

```
lastValidatedTimestamp increased
```

## `vt run pnpm-greet`

cache hit

```
$ pnpm --silent run greet ◉ cache hit, replaying
hello

---
vt run: cache hit.
```
