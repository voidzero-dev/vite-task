# vt_tui

The terminal UI of `vp run`: a live tree of the run's tasks in the primary screen, and a full-screen view of one task's output with scrollback, search, mouse selection and line input.

The crate does no I/O. The caller builds a `TaskTree`, creates a `Tui`, and feeds it the run's events (`set_state`, `set_status`, `task_output`), the terminal's input (`input`, or `input_event` where input isn't a byte stream, as on Windows; `resize`) and the time (`tick`). It writes what `render` returns to the terminal and acts on `take_requests`, such as sending a typed line to a task. `finish` prints the report that stays after the UI is gone.

Each task's output is kept in an `alacritty_terminal` emulator, pinned to a git commit because the released crate doesn't build for WebAssembly. `terminput` parses the terminal's input and `ratatui-core` lays out frames.

`crates/vt_tui_demo` drives this crate with scripted runs in the browser; see its README for the RFC page.
