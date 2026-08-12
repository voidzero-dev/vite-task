# tracks_inputs_on_linux_and_windows

TypeScript 7's native compiler reads source files through system calls that fspy can observe on Linux and Windows. Changing a source file should therefore invalidate the automatically inferred input fingerprint.

## `tsc --version`

```
Version 7.0.2
```

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
