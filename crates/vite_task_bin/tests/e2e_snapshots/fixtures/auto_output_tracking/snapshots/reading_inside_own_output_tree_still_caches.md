# reading_inside_own_output_tree_still_caches

A compound command's `&&` segments are separate cached tasks. Here the middle segment reads a file inside its own package's output directory while the last segment writes another file there, which is the shape of emdash's admin build: `tsdown` reads `dist/locales/<locale>/messages.mjs` and a later `locale:copy` step rewrites those files. Fingerprinting content a sibling task rewrites can never settle, so a read is treated as the task's own derived state when version control calls it derived *and* it sits under a directory the same task wrote into. Requiring both is what keeps `node_modules/<dep>/dist` a real input, so changing a dependency still invalidates its consumers.

## `vt run task`

```
~/packages/sibling-pkg$ vtt publish-dir rebuild src/data.txt dist

~/packages/sibling-pkg$ vtt cp dist/out.txt dist/copied.txt

~/packages/sibling-pkg$ vtt cp src/data.txt dist/late.txt

---
vt run: 0/3 cache hit (0%). (Run `vt run --last-details` for full details)
```

## `vt run task`

```
~/packages/sibling-pkg$ vtt publish-dir rebuild src/data.txt dist ◉ cache hit, replaying

~/packages/sibling-pkg$ vtt cp dist/out.txt dist/copied.txt ◉ cache hit, replaying

~/packages/sibling-pkg$ vtt cp src/data.txt dist/late.txt ◉ cache hit, replaying

---
vt run: 3/3 cache hit (100%). (Run `vt run --last-details` for full details)
```

## `vt run task`

```
~/packages/sibling-pkg$ vtt publish-dir rebuild src/data.txt dist ◉ cache hit, replaying

~/packages/sibling-pkg$ vtt cp dist/out.txt dist/copied.txt ◉ cache hit, replaying

~/packages/sibling-pkg$ vtt cp src/data.txt dist/late.txt ◉ cache hit, replaying

---
vt run: 3/3 cache hit (100%). (Run `vt run --last-details` for full details)
```
