//! Drives the UI and reads back what a terminal would show.

use std::time::Duration;

use alacritty_terminal::{
    event::VoidListener,
    grid::Dimensions,
    index::{Column, Line},
    term::{Config, Term},
    vte::ansi::Processor,
};

use crate::{NodeSpec, Options, Request, TaskState, TaskTree, Tone, Tui};

struct Size(usize, usize);

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.1
    }

    fn screen_lines(&self) -> usize {
        self.1
    }

    fn columns(&self) -> usize {
        self.0
    }
}

/// The terminal the UI draws in.
struct Screen {
    term: Term<VoidListener>,
    parser: Processor,
}

impl Screen {
    fn new(cols: usize, rows: usize) -> Self {
        Self {
            term: Term::new(Config::default(), &Size(cols, rows), VoidListener),
            parser: Processor::new(),
        }
    }

    fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    /// The visible rows, without trailing spaces or trailing blank rows.
    fn rows(&self) -> Vec<vt_str::Str> {
        let grid = self.term.grid();
        let mut rows: Vec<vt_str::Str> = (0..self.term.screen_lines())
            .map(|y| {
                let row = &grid[Line(i32::try_from(y).unwrap())];
                let mut text = vt_str::Str::default();
                for x in 0..self.term.columns() {
                    text.push(row[Column(x)].c);
                }
                vt_str::Str::from(text.trim_end())
            })
            .collect();
        while rows.last().is_some_and(|row| row.is_empty()) {
            rows.pop();
        }
        rows
    }
}

fn build() -> (TaskTree, [crate::NodeId; 4]) {
    let mut tree = TaskTree::new();
    let app = tree.add(None, NodeSpec::new("app#build").command("vp build"));
    let lib = tree.add(None, NodeSpec::new("lib#build"));
    let tsc = tree.add(Some(lib), NodeSpec::new("tsc"));
    let dev = tree.add(None, NodeSpec::new("docs#dev").command("vp dev").accepts_input(true));
    (tree, [app, lib, tsc, dev])
}

/// The layout people see, with a still icon so frames don't depend on time.
fn options() -> Options {
    let mut options = Options::new(60, 10, "brave-otter").for_agent();
    options.decorations = true;
    options.frame_interval = Duration::ZERO;
    options.first_frame_delay = Duration::from_millis(100);
    options
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|window| window == needle)
}

fn frame(tui: &mut Tui, screen: &mut Screen, now: u64) {
    tui.tick(Duration::from_millis(now));
    screen.feed(&tui.render());
}

#[test]
fn nothing_is_drawn_before_the_first_frame_delay() {
    let (tree, _) = build();
    let mut tui = Tui::new(tree, options());
    tui.tick(Duration::from_millis(50));
    assert!(tui.render().is_empty());
}

#[test]
fn the_tree_updates_in_place() {
    let (tree, [app, _, tsc, dev]) = build();
    let mut tui = Tui::new(tree, options());
    let mut screen = Screen::new(60, 10);
    screen.feed(b"$ vp run -r build\r\n");
    tui.set_state(app, TaskState::Running);
    tui.set_status(app, Some("812 modules"), Tone::Normal);
    frame(&mut tui, &mut screen, 100);
    assert_eq!(
        screen.rows(),
        [
            "$ vp run -r build",
            "  ● app#build: vp build",
            "  ○ lib#build",
            "    ○ tsc",
            "  ○ docs#dev: vp dev",
            "  ↑↓ select a task   ctrl-c stop                 brave-otter",
        ]
    );

    tui.tick(Duration::from_millis(1_300));
    tui.set_state(app, TaskState::Succeeded);
    tui.set_status(app, Some("812 modules"), Tone::Normal);
    tui.set_state(tsc, TaskState::Running);
    tui.set_state(dev, TaskState::Ready);
    tui.set_status(dev, Some("http://localhost:5173"), Tone::Normal);
    frame(&mut tui, &mut screen, 1_300);
    assert_eq!(
        screen.rows(),
        [
            "$ vp run -r build",
            "  ✓ app#build: vp build  812 modules · 1.3s",
            "  ● lib#build",
            "    ● tsc",
            "  ◉ docs#dev: vp dev     http://localhost:5173",
            "  ↑↓ select a task   ctrl-c stop                 brave-otter",
        ]
    );
}

#[test]
fn finishing_prints_failures_below_the_last_frame() {
    let (tree, [app, _, tsc, dev]) = build();
    let mut tui = Tui::new(tree, options());
    let mut screen = Screen::new(60, 20);
    tui.set_state(app, TaskState::CacheHit);
    tui.set_state(tsc, TaskState::Running);
    tui.task_output(tsc, b"src/a.ts(1,7): error TS2322\n");
    frame(&mut tui, &mut screen, 100);
    tui.set_state(tsc, TaskState::Failed);
    tui.set_state(dev, TaskState::NotRun);
    tui.tick(Duration::from_millis(600));
    tui.finish(&b"vp run: lib#build failed\r\n"[..]);
    screen.feed(&tui.render());
    assert!(tui.is_done());
    assert_eq!(
        screen.rows(),
        [
            "✓ app#build: vp build  cache hit",
            "✗ lib#build            100ms",
            "  ✗ tsc                100ms",
            "– docs#dev: vp dev     not run",
            "",
            "✗ lib#build › tsc ──────────────────────────────────────────",
            "src/a.ts(1,7): error TS2322",
            "vp run: lib#build failed",
        ]
    );
}

#[test]
fn typed_lines_go_to_the_task() {
    let (tree, [.., dev]) = build();
    let mut tui = Tui::new(tree, options());
    let mut screen = Screen::new(60, 10);
    tui.set_state(dev, TaskState::Ready);
    tui.task_output(dev, b"press h + enter to show help\n");
    frame(&mut tui, &mut screen, 100);
    // Select the dev server, open it, and type `h` + enter.
    tui.input(b"\x1b[B");
    tui.input(b"\x1b[B\x1b[B\x1b[B\r");
    // Typed text shows at the task's cursor, and nothing is sent before Enter.
    tui.input(b"h");
    frame(&mut tui, &mut screen, 150);
    assert_eq!(screen.rows()[..2], ["press h + enter to show help", "h"]);
    assert!(tui.take_requests().is_empty());
    tui.input(b"\r");
    assert_eq!(tui.take_requests(), [Request::Input { task: dev, data: b"h\n".to_vec() }]);
    frame(&mut tui, &mut screen, 200);
    let rows = screen.rows();
    assert_eq!(rows[..2], ["press h + enter to show help", "h"]);
    assert!(rows.last().unwrap().contains("esc tree"), "{rows:?}");
}

#[test]
fn ctrl_c_asks_to_cancel_the_run() {
    let (tree, _) = build();
    let mut tui = Tui::new(tree, options());
    tui.input(b"\x03");
    assert_eq!(tui.take_requests(), [Request::CancelRun]);
}

#[test]
fn input_events_work_like_bytes() {
    use terminput::{Event, KeyCode, KeyEvent, KeyModifiers};

    let (tree, _) = build();
    let mut tui = Tui::new(tree, options());
    tui.input_event(Event::Key(KeyEvent::new(KeyCode::Char('c')).modifiers(KeyModifiers::CTRL)));
    assert_eq!(tui.take_requests(), [Request::CancelRun]);
}

#[test]
fn restoring_leaves_the_task_view() {
    let (tree, [app, ..]) = build();
    let mut tui = Tui::new(tree, options());
    let mut screen = Screen::new(60, 10);
    tui.set_state(app, TaskState::Running);
    frame(&mut tui, &mut screen, 100);
    assert_eq!(tui.restore_sequence(), b"\x1b[?2026l\r\n\x1b[?25h");
    tui.input(b"\x1b[B\r");
    frame(&mut tui, &mut screen, 200);
    assert_eq!(
        tui.restore_sequence(),
        b"\x1b[?2026l\x1b[?1006l\x1b[?1002l\x1b[?1000l\x1b[?1049l\r\n\x1b[?25h"
    );
    tui.input(b"\x1b");
    tui.finish(&b""[..]);
    let _ = tui.render();
    assert!(tui.is_done());
    assert!(tui.restore_sequence().is_empty());
}

#[test]
fn long_labels_push_their_status_instead_of_being_cut() {
    let mut tree = TaskTree::new();
    let short = tree.add(None, NodeSpec::new("a#build").command("tsc"));
    let long =
        tree.add(None, NodeSpec::new("@acme/web#codegen").command("graphql-codegen --watch"));
    let mut tui = Tui::new(tree, options());
    let mut screen = Screen::new(60, 10);
    tui.set_state(short, TaskState::CacheHit);
    tui.set_state(long, TaskState::CacheHit);
    frame(&mut tui, &mut screen, 100);
    assert_eq!(
        screen.rows()[..2],
        [
            "  ✓ a#build: tsc                    cache hit",
            "  ✓ @acme/web#codegen: graphql-codegen --watch  cache hit",
        ]
    );
}

#[test]
fn agent_mode_writes_only_what_an_agent_reads() {
    let (tree, [app, _, tsc, dev]) = build();
    let mut options = Options::new(60, 10, "brave-otter").for_agent();
    options.first_frame_delay = Duration::from_millis(100);
    let mut tui = Tui::new(tree, options);
    let mut screen = Screen::new(60, 10);
    tui.set_state(app, TaskState::Running);
    tui.set_status(app, Some("812 modules"), Tone::Normal);
    tui.tick(Duration::from_millis(100));
    let first = tui.render();
    screen.feed(&first);
    // No styles, no synchronized updates, no key hints, no selection column,
    // and statuses follow their labels.
    assert!(!contains(&first, b"\x1b[0;"), "{first:?}");
    assert!(!contains(&first, b"\x1b[?2026"), "{first:?}");
    assert_eq!(
        screen.rows(),
        ["● app#build: vp build", "○ lib#build", "  ○ tsc", "○ docs#dev: vp dev"]
    );

    // Changes within the frame interval wait for the next frame.
    tui.set_state(app, TaskState::Succeeded);
    frame(&mut tui, &mut screen, 1_000);
    assert_eq!(screen.rows()[0], "● app#build: vp build");
    frame(&mut tui, &mut screen, 2_100);
    assert_eq!(screen.rows()[0], "✓ app#build: vp build  812 modules · 100ms");

    // Input is answered at once, and selecting brings the selection column.
    tui.set_state(tsc, TaskState::Failed);
    tui.tick(Duration::from_millis(2_200));
    tui.input(b"\x1b[B");
    screen.feed(&tui.render());
    assert_eq!(screen.rows()[1], "  ✗ lib#build  0ms");

    tui.set_state(dev, TaskState::NotRun);
    tui.task_output(tsc, b"\x1b[31merror\x1b[39m TS2322\n");
    tui.finish(&b"\x1b[2m---\x1b[22m\r\nvp run: 1 failed\r\n"[..]);
    let last = tui.render();
    screen.feed(&last);
    assert!(!contains(&last, b"\x1b[0;"), "{last:?}");
    assert!(!contains(&last, b"\x1b[31m"), "{last:?}");
    assert_eq!(
        screen.rows()[4..],
        ["", "✗ lib#build › tsc", "error TS2322", "---", "vp run: 1 failed"]
    );
}

#[test]
fn the_task_view_sends_ctrl_c_to_the_task() {
    let (tree, [app, ..]) = build();
    let mut tui = Tui::new(tree, options());
    let mut screen = Screen::new(60, 10);
    tui.set_state(app, TaskState::Running);
    frame(&mut tui, &mut screen, 100);
    tui.input(b"\x1b[B\r");
    // app#build reads no input: typing to it sends nothing.
    tui.input(b"x\r");
    assert!(tui.take_requests().is_empty());
    tui.input(b"\x03");
    assert_eq!(tui.take_requests(), [Request::Interrupt { task: app }]);
    // Once the task has ended, Ctrl-C stops the run.
    tui.set_state(app, TaskState::Cancelled);
    tui.input(b"\x03");
    assert_eq!(tui.take_requests(), [Request::CancelRun]);
}
