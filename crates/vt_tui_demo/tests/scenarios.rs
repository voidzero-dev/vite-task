//! Plays the demo's runs to the end in a virtual terminal.

use std::time::Duration;

use alacritty_terminal::{
    event::VoidListener,
    grid::Dimensions,
    index::{Column, Line},
    term::{Config, Term},
    vte::ansi::Processor,
};
use vt_tui::Options;
use vt_tui_demo::{Demo, Plain, Player};

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

struct Screen {
    term: Term<VoidListener>,
    parser: Processor,
}

impl Screen {
    fn new() -> Self {
        let config = Config { scrolling_history: 1000, ..Config::default() };
        Self { term: Term::new(config, &Size(100, 30), VoidListener), parser: Processor::new() }
    }

    fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    /// The whole buffer, history included, as text.
    fn text(&self) -> vt_str::Str {
        let grid = self.term.grid();
        let mut text = vt_str::Str::default();
        let top = self.term.topmost_line().0;
        let bottom = self.term.bottommost_line().0;
        for line in top..=bottom {
            let row = &grid[Line(line)];
            let mut row_text = vt_str::Str::default();
            for x in 0..self.term.columns() {
                row_text.push(row[Column(x)].c);
            }
            text.push_str(row_text.trim_end());
            text.push('\n');
        }
        vt_str::Str::from(text.trim_end())
    }
}

fn play(demo: &mut Demo, screen: &mut Screen, from: u32, to: u32) {
    for step in from / 50..to / 50 {
        screen.feed(&demo.frame(f64::from(step * 50)));
    }
}

#[test]
fn verify_run_fails_and_prints_the_failing_test() {
    let mut demo = Demo::new("verify", 100, 30, false, true, 7);
    let mut screen = Screen::new();
    play(&mut demo, &mut screen, 0, 30_000);
    assert!(!demo.running());
    let text = screen.text();
    assert!(text.contains("✗ @acme/web#test: vitest run"), "{text}");
    assert!(text.contains("AssertionError"), "{text}");
    assert!(text.contains("vp run: 2/11 cache hit"), "{text}");
}

#[test]
fn dev_run_stops_on_ctrl_c() {
    let mut demo = Demo::new("dev", 100, 30, false, false, 7);
    let mut screen = Screen::new();
    play(&mut demo, &mut screen, 0, 4_000);
    assert!(screen.text().contains("http://localhost:5173"));
    demo.input(b"\x03");
    play(&mut demo, &mut screen, 4_000, 4_200);
    assert!(!demo.running());
    let text = screen.text();
    assert!(text.contains("stopped by Ctrl-C"), "{text}");
}

#[test]
fn plain_verify_run_labels_each_line() {
    let mut run = Plain::new(vt_tui_demo::verify(true), "brave-otter");
    let mut out = Vec::new();
    for step in 0..600 {
        out.extend(run.frame(Duration::from_millis(step * 50)));
    }
    assert!(run.is_done());
    assert_eq!(run.exit_code(), 1);
    let text = std::str::from_utf8(&out).unwrap();
    assert!(text.contains("\x1b[90m[acme#verify]\x1b[39m Found \x1b[33m2 warnings"), "{text}");
    assert!(
        text.contains("\x1b[90m[@acme/web#test]\x1b[39m \x1b[31m\x1b[1mAssertionError"),
        "{text}"
    );
    assert!(text.contains("vp run: 2/11 cache hit (18%), \x1b[31m1 failed"), "{text}");
    assert!(!text.contains('\r'), "{text:?}");
}

#[test]
fn exit_codes() {
    let mut passing =
        Player::new(vt_tui_demo::verify(false), Options::new(100, 30, "x").for_agent());
    for step in 0..600 {
        let _ = passing.frame(Duration::from_millis(step * 50));
    }
    assert!(passing.is_done());
    assert_eq!(passing.exit_code(), 0);

    let mut dev = Player::new(vt_tui_demo::dev(), Options::new(100, 30, "x"));
    let _ = dev.frame(Duration::from_secs(1));
    dev.cancel(Duration::from_secs(1));
    let _ = dev.frame(Duration::from_secs(1));
    assert!(dev.is_done());
    assert_eq!(dev.exit_code(), 130);
}
