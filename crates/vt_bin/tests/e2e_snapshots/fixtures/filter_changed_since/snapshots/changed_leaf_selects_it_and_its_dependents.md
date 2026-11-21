# changed_leaf_selects_it_and_its_dependents

A commit changes only the leaf `@test/utils`. `...[HEAD~1]` selects it and every package that depends on it, and builds them in dependency order.

## `git -c init.defaultBranch=main init -q`

```
```

## `git -c core.autocrlf=false add -A`

```
```

## `git -c user.name=test -c user.email=test@example.com -c commit.gpgsign=false commit -q --no-verify -m init`

```
```

## `vtt replace-file-content packages/utils/src/index.js 'export {};' 'export const changed = 1;'`

```
```

## `git -c core.autocrlf=false add -A`

```
```

## `git -c user.name=test -c user.email=test@example.com -c commit.gpgsign=false commit -q --no-verify -m change`

```
```

## `vt run --filter ...[HEAD~1] build`

```
~/packages/utils$ vtt print built-utils ⊘ cache disabled
built-utils

~/packages/lib$ vtt print built-lib ⊘ cache disabled
built-lib

~/packages/app$ vtt print built-app ⊘ cache disabled
built-app

---
vt run: 3/3 successful. (Run `vt run --last-details` for full details)
```

## `vt run --filter [HEAD~1] build`

```
~/packages/utils$ vtt print built-utils ⊘ cache disabled
built-utils
```
