# typescript_out_dir

TypeScript 7's `tsc` compiles `src` into `dist`. It stats `dist` and lists the package root.

## `vt run build`

```
$ tsc
```

## `vtt rm -r dist`

remove the output folder, as on a clean checkout

```
```

## `vt run build`

```
$ tsc ○ cache miss: 'dist' removed from workspace root, executing
```

## `vtt stat-file dist/main.js`

```
dist/main.js: exists
```
