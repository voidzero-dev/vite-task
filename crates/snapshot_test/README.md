# snapshot_test

Minimal snapshot testing primitive for this workspace. Two methods:

```rust
snapshots.check_snapshot(name, actual)?;              // raw text → {name}
snapshots.check_json_snapshot(name, comment, value)?; // JSON → {name}.jsonc with `// {comment}` header
```

Both return `Result<(), String>` — the `String` contains a unified diff on mismatch, pointing to a `.new` file. Set `UPDATE_SNAPSHOTS=1` to accept new output in-place.

## Why not `insta`?

`insta::assert_*!` panics on mismatch and prints the diff to stderr. When run under `libtest-mimic` (which doesn't capture stdout/stderr), that diff appears in the middle of the test-runner output stream, _not_ inside the `failures:` summary section. The summary only shows the panic message (`snapshot assertion for 'X' failed in line N`). Returning `Result<(), String>` lets us put the full diff directly into the failure message where `cargo test` prints it at the end.
