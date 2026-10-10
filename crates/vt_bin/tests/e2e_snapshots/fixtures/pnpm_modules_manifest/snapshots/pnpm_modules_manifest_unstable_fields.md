# pnpm_modules_manifest_unstable_fields

Fields of pnpm's `node_modules/.modules.yaml` that vary with time or with the machine are left out of its fingerprint, so changing them keeps the cache hit. Changing any other field is a cache miss.

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

## `vtt replace-file-content node_modules/.modules.yaml /home/alice/.local/share/pnpm/store/v11 /home/bob/.local/share/pnpm/store/v11`

another machine: new storeDir

```
```

## `vtt replace-file-content node_modules/.modules.yaml '".pnpm"' '"../../home/bob/.local/share/pnpm/store/v11/links"'`

global virtual store: new virtualStoreDir

```
```

## `vtt replace-file-content node_modules/.modules.yaml '".claude/skills/a"' '"/home/bob/skills/a"'`

skill outside the workspace: new linkedSkills

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

## `vtt replace-file-content node_modules/.modules.yaml pnpm@11.24.0 pnpm@11.25.0`

new packageManager

```
```

## `vt run read-manifest`

cache miss: 'node_modules/.modules.yaml' modified

```
$ vtt print-file node_modules/.modules.yaml ○ cache miss: 'node_modules/.modules.yaml' modified, executing
{
  "packageManager": "pnpm@11.25.0",
  "prunedAt": "Fri, 09 Oct 2026 03:13:58 GMT",
  "storeDir": "/home/bob/.local/share/pnpm/store/v11",
  "virtualStoreDir": "../../home/bob/.local/share/pnpm/store/v11/links",
  "linkedSkills": ["/home/bob/skills/a"]
}
```
