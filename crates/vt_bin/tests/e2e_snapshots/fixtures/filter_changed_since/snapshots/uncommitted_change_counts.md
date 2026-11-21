# uncommitted_change_counts

An uncommitted change to a tracked file counts as a change since `HEAD`.

## `git -c init.defaultBranch=main init -q`

```
```

## `git -c core.autocrlf=false add -A`

```
```

## `git -c user.name=test -c user.email=test@example.com -c commit.gpgsign=false commit -q --no-verify -m init`

```
```

## `vtt replace-file-content packages/lib/src/index.js 'export {};' 'export const changed = 1;'`

```
```

## `vt run --filter [HEAD] build`

```
~/packages/lib$ vtt print built-lib ⊘ cache disabled
built-lib
```
