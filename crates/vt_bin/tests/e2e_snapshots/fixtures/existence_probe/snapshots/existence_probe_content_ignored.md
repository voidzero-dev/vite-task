# existence_probe_content_ignored

A task that only checks whether files exist, as Vite does while looking for the workspace root, depends on whether they exist and not on their content. Editing `pnpm-workspace.yaml` keeps the cache hit, and creating or removing `lerna.json` is a cache miss.

## `vt run find-root`

cache miss

```
~/packages/app$ vtt exists ../../pnpm-workspace.yaml ../../lerna.json
../../pnpm-workspace.yaml: exists
../../lerna.json: missing
```

## `vtt replace-file-content ../../pnpm-workspace.yaml 'vite: 8.0.0' 'vite: 8.0.0
  left-pad: 1.3.0'`

add an unused catalog entry

```
```

## `vt run find-root`

cache hit

```
~/packages/app$ vtt exists ../../pnpm-workspace.yaml ../../lerna.json ◉ cache hit, replaying
../../pnpm-workspace.yaml: exists
../../lerna.json: missing

---
vt run: cache hit.
```

## `vtt write-file ../../lerna.json {}`

create a root marker

```
```

## `vt run find-root`

cache miss: input added

```
~/packages/app$ vtt exists ../../pnpm-workspace.yaml ../../lerna.json ○ cache miss: 'lerna.json' added in workspace root, executing
../../pnpm-workspace.yaml: exists
../../lerna.json: exists
```

## `vtt rm ../../lerna.json`

remove the root marker

```
```

## `vt run find-root`

cache miss: input removed

```
~/packages/app$ vtt exists ../../pnpm-workspace.yaml ../../lerna.json ○ cache miss: 'lerna.json' removed from workspace root, executing
../../pnpm-workspace.yaml: exists
../../lerna.json: missing
```
