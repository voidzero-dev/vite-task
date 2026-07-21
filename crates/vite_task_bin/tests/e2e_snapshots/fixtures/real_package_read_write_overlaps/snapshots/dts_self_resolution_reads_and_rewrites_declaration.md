# dts_self_resolution_reads_and_rewrites_declaration

The source imports a type through its own package name, whose `exports.types` target is the existing `dist/index.d.ts`. rolldown-plugin-dts delegates that import to TypeScript's bundler resolver and creates a TypeScript program, so the declaration is resolved and read before the declaration bundle replaces the same path. See [`tscResolve`](https://github.com/sxzz/rolldown-plugin-dts/blob/363cc09b660675e15ec8791ce0f05f1044a7048a/src/tsc/resolver.ts#L7-L38) and [the TypeScript program construction](https://github.com/sxzz/rolldown-plugin-dts/blob/363cc09b660675e15ec8791ce0f05f1044a7048a/src/tsc/emit-compiler.ts#L112-L128).

## `vt run -v overlap`

```
~/cases/dts-self-resolution$ tsdown --logLevel silent


━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 tasks • 0 cache hits • 1 cache misses
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] dts-self-resolution-overlap#overlap: ~/cases/dts-self-resolution$ tsdown --logLevel silent ✓
      → Not cached: read and wrote 'cases/dts-self-resolution/dist/index.d.ts'
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```
