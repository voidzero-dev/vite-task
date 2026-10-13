# pnpm_modules_manifest_content_ignored

Only whether pnpm's `node_modules/.modules.yaml` exists is part of the fingerprint, so changing its content keeps the cache hit. Removing it is a cache miss.

## `vtt mkdir -p node_modules`

```
```

## `vtt cp modules.json node_modules/.modules.yaml`

```
```

## `vt run read-manifest`

cache miss

```
$ vtt print-file node_modules/.modules.yaml
{
  "packageManager": "pnpm@11.24.0",
  "prunedAt": "Thu, 08 Oct 2026 08:18:18 GMT",
  "storeDir": "/home/alice/.local/share/pnpm/store/v11",
  "virtualStoreDir": ".pnpm",
  "linkedSkills": [".claude/skills/a"]
}
```

## `vtt replace-file-content node_modules/.modules.yaml 'Thu, 08 Oct 2026 08:18:18 GMT' 'Fri, 09 Oct 2026 03:13:58 GMT'`

reinstall: new prunedAt

```
```

## `vtt replace-file-content node_modules/.modules.yaml pnpm@11.24.0 pnpm@11.25.0`

install with another pnpm version: new packageManager

```
```

## `vt run read-manifest`

cache hit

```
$ vtt print-file node_modules/.modules.yaml ◉ cache hit, replaying
{
  "packageManager": "pnpm@11.24.0",
  "prunedAt": "Thu, 08 Oct 2026 08:18:18 GMT",
  "storeDir": "/home/alice/.local/share/pnpm/store/v11",
  "virtualStoreDir": ".pnpm",
  "linkedSkills": [".claude/skills/a"]
}

---
vt run: cache hit.
```

## `vtt rm node_modules/.modules.yaml`

remove the manifest

```
```

## `vt run read-manifest`

cache miss: input removed

```
$ vtt print-file node_modules/.modules.yaml ○ cache miss: '.modules.yaml' removed from 'node_modules', executing
node_modules/.modules.yaml: not found
```
