# tsdown_rebuild_after_edit

tsdown bundles `src/index.ts` into `dist`. Before each build it lists `dist` and removes what it holds.

## `vt run build`

```
$ tsdown --log-level error
```

## `vtt replace-file-content src/index.ts initial edited`

edit the source, so the next run cleans and rebuilds the existing `dist`

```
```

## `vt run build`

```
$ tsdown --log-level error ○ cache miss: 'src/index.ts' modified, executing
```

## `vtt rm -r dist`

remove the output folder, as on a clean checkout

```
```

## `vt run build`

```
$ tsdown --log-level error ○ cache miss: 'dist' removed from workspace root, executing
```

## `vtt grep-file dist/index.mjs edited`

```
dist/index.mjs: found "edited"
```
