# pnpm_workspace_state_ignored

pnpm's `node_modules/.pnpm-workspace-state-v1.json` is neither an input nor an output, so a task that reads and rewrites it, like pnpm's dependency check before `pnpm run`, is cached, and changes to the file keep the cache hit.

## `vtt mkdir -p node_modules`

```
```

## `vtt cp workspace-state.json node_modules/.pnpm-workspace-state-v1.json`

```
```

## `vt run verify-deps`

cache miss: reads and rewrites the state

```
$ vtt replace-file-content node_modules/.pnpm-workspace-state-v1.json 1791447500251 1791447600000
```

## `vt run verify-deps`

cache hit

```
$ vtt replace-file-content node_modules/.pnpm-workspace-state-v1.json 1791447500251 1791447600000 ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vtt replace-file-content node_modules/.pnpm-workspace-state-v1.json /home/alice/project /home/bob/project`

install with the workspace at another path

```
```

## `vt run verify-deps`

cache hit

```
$ vtt replace-file-content node_modules/.pnpm-workspace-state-v1.json 1791447500251 1791447600000 ◉ cache hit, replaying

---
vt run: cache hit.
```
