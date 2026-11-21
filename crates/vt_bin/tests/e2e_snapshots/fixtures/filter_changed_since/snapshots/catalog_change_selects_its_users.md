# catalog_change_selects_its_users

A commit changes only the default catalog's `react` in `pnpm-workspace.yaml`. `@test/app`, which takes `react: catalog:`, counts as changed, and so does the root package that holds the file (it has no `build`).

## `git -c init.defaultBranch=main init -q`

```
```

## `git -c core.autocrlf=false add -A`

```
```

## `git -c user.name=test -c user.email=test@example.com -c commit.gpgsign=false commit -q --no-verify -m init`

```
```

## `vtt replace-file-content pnpm-workspace.yaml 'react: ^18.0.0' 'react: ^19.0.0'`

```
```

## `git -c core.autocrlf=false add -A`

```
```

## `git -c user.name=test -c user.email=test@example.com -c commit.gpgsign=false commit -q --no-verify -m change`

```
```

## `vt run --filter [HEAD~1] build`

```
~/packages/app$ vtt print built-app ⊘ cache disabled
built-app
```
