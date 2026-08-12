# tracks_inputs

TypeScript 7's native compiler reads source files through system calls that fspy can observe. Changing a source file should therefore invalidate the automatically inferred input fingerprint.

## `vt run typecheck`

```
$ tsc --noEmit
```

## `vtt replace-file-content src/main.ts initial modified`

```
```

## `vt run typecheck`

```
$ tsc --noEmit ○ cache miss: 'src/main.ts' modified, executing
```
