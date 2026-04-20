# subpackage_glob_with_cwd___glob_relative_to_package_not_cwd

A custom `cwd` on a subpackage task should not shift its glob base — `src/**`
still resolves relative to the subpackage directory.

## `vt run sub-pkg#sub-glob-with-cwd`

```
~/packages/sub-pkg/src$ vtt print-file sub.ts
export const sub = 'initial';
```

## `vtt replace-file-content packages/sub-pkg/src/sub.ts initial modified`

```
```

## `vt run sub-pkg#sub-glob-with-cwd`

```
~/packages/sub-pkg/src$ vtt print-file sub.ts ○ cache miss: 'packages/sub-pkg/src/sub.ts' modified, executing
export const sub = 'modified';
```
