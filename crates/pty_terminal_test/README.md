# pty_terminal_test

`pty_terminal_test` is a thin test helper on top of `pty_terminal` for writing
integration tests against interactive CLI processes.

It provides:

- `TestTerminal::spawn(...)` to start a child process in a PTY.
- `writer` (`PtyWriter`) to send input to the child.
- `reader` (`Reader`) to wait for milestones and collect final exit status.

## Why this crate exists

Reading raw PTY bytes is often not enough for deterministic interactive tests.
You usually need explicit synchronization points from the child process.

This crate solves that by pairing:

- `pty_terminal_test_client::mark_milestone("name")` in the child process, and
- `reader.expect_milestone("name")` in the test process.

## Core API

```rust
use portable_pty::CommandBuilder;
use pty_terminal::geo::ScreenSize;
use pty_terminal_test::TestTerminal;

let cmd = CommandBuilder::from("your-binary-or-subprocess-test-command");
let TestTerminal { mut writer, mut reader, child_handle: _ } =
    TestTerminal::spawn(ScreenSize { rows: 80, cols: 80 }, cmd)?;

// Wait until child reaches a known point.
let _screen = reader.expect_milestone("ready");

// Interact with child.
writer.write_all(b"q")?;
writer.flush()?;

// Wait for completion.
let status = reader.wait_for_exit();
assert!(status.success());
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Milestone protocol

Milestones are encoded as unique window titles:

```text
pty-terminal-test:<32-hex-random-id>:<name-base64url>
```

`Reader::expect_milestone` works like this:

1. Drain title events captured by `PtyReader`.
2. Ignore ordinary titles and completed non-target milestones.
3. If no match exists, continue reading from the PTY and repeat.
4. Return the current screen once the requested title is observed.

## Cross-platform behavior

On Windows the client calls `SetConsoleTitleW`; ConPTY emits the resulting title
through its asynchronous renderer after preceding text and cursor state. On Unix
the client emits an OSC 2 title update, which follows normal PTY byte ordering.
The same token decoder and test API are used on every platform.

## Typical test pattern

In the child process:

```rust
pty_terminal_test_client::mark_milestone("ready");
// do work...
pty_terminal_test_client::mark_milestone("after-input");
```

In the parent test:

```rust
let _ = reader.expect_milestone("ready");
writer.write_all(b"input")?;
writer.flush()?;
let screen = reader.expect_milestone("after-input");
```
