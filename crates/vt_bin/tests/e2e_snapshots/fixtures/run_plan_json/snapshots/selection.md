# selection

A task missing from some of the selected packages is left out of the plan. Missing from all of them, it's an error, but a filter that matches no package gives an empty plan, unless `--fail-if-no-match` is set.

## `vtt pipe-stdout -- vt run --dry-run --json -r steps`

Only `@acme/web` has `steps`.

```
--- stdout ---
{"$schema":"<schema>","version":1,"workspaceRoot":"<workspace>","plan":{"packages/web#steps":{"package":"packages/web","name":"steps","steps":[{"program":"<vtt>","args":["print","one"],"cache":{"status":"noBaseline"}},{"program":"echo","args":["two"]}]}}}
```

## `vt run --dry-run --json --filter @acme/ui steps`

The only selected package doesn't have it.

**Exit code:** 1

```
error: Task "steps" not found
```

## `vt run --dry-run --json -r typo`

No package has it.

**Exit code:** 1

```
error: Task "typo" not found
```

## `vtt pipe-stdout -- vt run --dry-run --json nope#build`

No package is named `nope`, so, as in a real run, the plan is empty.

```
--- stdout ---
{"$schema":"<schema>","version":1,"workspaceRoot":"<workspace>","plan":{}}
```

## `vtt pipe-stdout -- vt run --dry-run --json --filter nope build`

No package matched, so the plan is empty.

```
No packages matched the filter: nope
--- stdout ---
{"$schema":"<schema>","version":1,"workspaceRoot":"<workspace>","plan":{}}
```

## `vt run --dry-run --json --fail-if-no-match --filter nope build`

`--fail-if-no-match` makes that an error.

**Exit code:** 1

```
error: No packages matched the filter: nope
```
