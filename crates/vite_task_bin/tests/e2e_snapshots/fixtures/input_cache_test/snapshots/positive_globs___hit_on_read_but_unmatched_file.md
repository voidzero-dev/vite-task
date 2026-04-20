# positive_globs___hit_on_read_but_unmatched_file

With only positive globs (no `auto`), files read at runtime but not matched
by the glob should not be fingerprinted — inference is truly disabled.

## `vt run positive-globs-reads-unmatched`

```
$ vtt print-file src/main.ts src/utils.ts
export const main = 'initial';
export const utils = 'initial';
```

## `vtt replace-file-content src/utils.ts initial modified`

```
```

## `vt run positive-globs-reads-unmatched`

```
$ vtt print-file src/main.ts src/utils.ts ◉ cache hit, replaying
export const main = 'initial';
export const utils = 'initial';

---
vt run: cache hit.
```
