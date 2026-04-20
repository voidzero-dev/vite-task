# root_glob_with_cwd___glob_relative_to_package_not_cwd

Even when the task declares a custom `cwd`, its glob stays anchored at the
package root — `src/**` still matches `src/root.ts`.

## `vt run root-glob-with-cwd`

```
~/src$ vtt print-file root.ts
export const root = 'initial';
```

## `vtt replace-file-content src/root.ts initial modified`

```
```

## `vt run root-glob-with-cwd`

```
~/src$ vtt print-file root.ts ○ cache miss: 'src/root.ts' modified, executing
export const root = 'modified';
```
