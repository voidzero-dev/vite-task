# existence_probe_read_tracks_content

A task that reads `pnpm-workspace.yaml` still depends on its content, so editing it is a cache miss.

## `vt run read-manifest`

cache miss

```
~/packages/app$ vtt print-file ../../pnpm-workspace.yaml
packages:
  - packages/*

catalog:
  vite: 8.0.0
```

## `vtt replace-file-content ../../pnpm-workspace.yaml 'vite: 8.0.0' 'vite: 8.0.0
  left-pad: 1.3.0'`

add an unused catalog entry

```
```

## `vt run read-manifest`

cache miss: input modified

```
~/packages/app$ vtt print-file ../../pnpm-workspace.yaml ○ cache miss: 'pnpm-workspace.yaml' modified, executing
packages:
  - packages/*

catalog:
  vite: 8.0.0
  left-pad: 1.3.0
```
