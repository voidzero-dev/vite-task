//! The browser page's terminal: a run played into the UI, behind a tiny
//! shell that can start it again and show `vp run --last-details`.

use std::time::Duration;

use vt_str::Str;
use vt_tui::Options;
use wasm_bindgen::prelude::wasm_bindgen;

use crate::{names::RunNames, player::Player, scenarios};

const PROMPT: &str = "\x1b[36m~/acme\x1b[39m \x1b[35m❯\x1b[39m ";

struct Run {
    player: Player,
    /// When the run started, in the page's milliseconds; negative until the
    /// first frame.
    start: f64,
}

struct Finished {
    command: &'static str,
    player: Player,
}

/// A terminal session in the demo page.
#[wasm_bindgen]
pub struct Demo {
    scenario: Str,
    fail: bool,
    agent: bool,
    cols: u16,
    rows: u16,
    names: RunNames,
    command: &'static str,
    run: Option<Run>,
    history: Vec<Finished>,
    line: Str,
    out: Vec<u8>,
    now: f64,
}

#[wasm_bindgen]
impl Demo {
    /// Starts a session that runs `scenario` (`"dev"` or `"verify"`) right away.
    #[wasm_bindgen(constructor)]
    #[must_use]
    pub fn new(scenario: &str, cols: u16, rows: u16, agent: bool, fail: bool, seed: u32) -> Self {
        let mut demo = Self {
            scenario: Str::from(scenario),
            fail,
            agent,
            cols,
            rows,
            names: RunNames::new(seed),
            command: "",
            run: None,
            history: Vec::new(),
            line: Str::default(),
            out: Vec::new(),
            now: 0.0,
        };
        demo.command = demo.build().command;
        demo.out.extend_from_slice(PROMPT.as_bytes());
        demo.out.extend_from_slice(demo.command.as_bytes());
        demo.out.extend_from_slice(b"\r\n");
        demo.start(None);
        demo
    }

    /// Whether the UI is running, as opposed to the demo's shell prompt.
    #[must_use]
    #[expect(clippy::missing_const_for_fn, reason = "wasm-bindgen can't export const functions")]
    pub fn running(&self) -> bool {
        self.run.is_some()
    }

    /// Handles bytes the terminal sent.
    pub fn input(&mut self, data: &[u8]) {
        if let Some(run) = &mut self.run {
            run.player.input(data, since(run.start, self.now));
            return;
        }
        self.shell_input(data);
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.cols = cols;
        self.rows = rows;
        if let Some(run) = &mut self.run {
            run.player.resize(cols, rows);
        }
    }

    /// Advances to `now` (milliseconds) and returns the bytes to write.
    pub fn frame(&mut self, now: f64) -> Vec<u8> {
        self.now = now;
        if let Some(run) = &mut self.run {
            if run.start < 0.0 {
                run.start = now;
            }
            self.out.extend_from_slice(&run.player.frame(since(run.start, now)));
            if run.player.is_done()
                && let Some(run) = self.run.take()
            {
                self.history.push(Finished { command: self.command, player: run.player });
                self.out.extend_from_slice(PROMPT.as_bytes());
            }
        }
        std::mem::take(&mut self.out)
    }
}

impl Demo {
    fn build(&self) -> scenarios::Scenario {
        if self.scenario == "dev" { scenarios::dev() } else { scenarios::verify(self.fail) }
    }

    fn start(&mut self, at: Option<f64>) {
        let mut options = Options::new(self.cols, self.rows, self.names.next_name());
        if self.agent {
            options = options.for_agent();
        }
        self.run =
            Some(Run { player: Player::new(self.build(), options), start: at.unwrap_or(-1.0) });
    }

    fn shell_input(&mut self, data: &[u8]) {
        let text = std::str::from_utf8(data).unwrap_or_default();
        for c in text.chars() {
            match c {
                '\r' | '\n' => {
                    self.out.extend_from_slice(b"\r\n");
                    let line = std::mem::take(&mut self.line);
                    self.execute(line.trim());
                    if self.run.is_some() {
                        return;
                    }
                }
                '\u{7f}' | '\u{8}' => {
                    if self.line.pop().is_some() {
                        self.out.extend_from_slice(b"\x08 \x08");
                    }
                }
                '\u{3}' => {
                    self.line = Str::default();
                    self.out.extend_from_slice(b"^C\r\n");
                    self.out.extend_from_slice(PROMPT.as_bytes());
                }
                '\u{1b}' => {
                    // Arrow keys and other sequences do nothing at this prompt.
                    return;
                }
                c if !c.is_control() => {
                    self.line.push(c);
                    let mut utf8 = [0; 4];
                    self.out.extend_from_slice(c.encode_utf8(&mut utf8).as_bytes());
                }
                _ => {}
            }
        }
    }

    fn execute(&mut self, line: &str) {
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.as_slice() {
            [] => {}
            ["clear"] => self.out.extend_from_slice(b"\x1b[2J\x1b[3J\x1b[H"),
            ["vp", "run", "--last-details", rest @ ..] if rest.len() <= 1 => {
                self.last_details(rest.first().copied());
            }
            _ if line == self.command => {
                self.start(Some(self.now));
                return;
            }
            _ => {
                let message = vt_str::format!(
                    "\x1b[2mThis demo knows `{}`, `vp run --last-details [name]` and `clear`.\x1b[22m\r\n",
                    self.command
                );
                self.out.extend_from_slice(message.as_bytes());
            }
        }
        self.out.extend_from_slice(PROMPT.as_bytes());
    }

    fn last_details(&mut self, name: Option<&str>) {
        let found = match name {
            None => self.history.last(),
            Some(name) => self.history.iter().rev().find(|finished| finished.player.name() == name),
        };
        if let Some(finished) = found {
            let header = vt_str::format!(
                "\x1b[2mRun {} · {}\x1b[22m\r\n",
                finished.player.name(),
                finished.command
            );
            self.out.extend_from_slice(header.as_bytes());
            let details = finished.player.last_details();
            self.out.extend_from_slice(&details);
            return;
        }
        let mut message = Str::from("\x1b[31merror\x1b[39m: ");
        match name {
            Some(name) => message.push_str(&vt_str::format!("no saved run is named `{name}`.")),
            None => message.push_str("no run has finished yet."),
        }
        if !self.history.is_empty() {
            message.push_str(" Recent runs:");
            for finished in self.history.iter().rev().take(5) {
                message.push_str(&vt_str::format!(
                    "\r\n  {}  \x1b[2m{}\x1b[22m",
                    finished.player.name(),
                    finished.command
                ));
            }
        }
        message.push_str("\r\n");
        self.out.extend_from_slice(message.as_bytes());
    }
}

/// The time from `start` to `now`, both in milliseconds.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "milliseconds since the run started"
)]
const fn since(start: f64, now: f64) -> Duration {
    Duration::from_millis((now - start).max(0.0) as u64)
}
