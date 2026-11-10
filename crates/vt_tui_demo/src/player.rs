//! One run of a scenario played into the UI, in step with a clock.

use std::time::Duration;

use vt_str::Str;
use vt_tui::{InputEvent, Options, Request, Tui};

use crate::{scenarios::Scenario, sim::Sim};

/// A scenario's run in the UI. Feed it the terminal's input and the time since
/// the run started, and write the bytes [`Player::frame`] returns.
#[derive(Debug)]
pub struct Player {
    name: Str,
    tui: Tui,
    sim: Sim,
    summary: Option<Str>,
}

impl Player {
    #[must_use]
    pub fn new(scenario: Scenario, options: Options) -> Self {
        let name = options.run_name.clone();
        Self { name, tui: Tui::new(scenario.tree, options), sim: scenario.sim, summary: None }
    }

    /// The name `vp run --last-details` takes to show this run again.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Handles bytes the terminal sent. `now` is the time since the run
    /// started.
    pub fn input(&mut self, data: &[u8], now: Duration) {
        self.tui.input(data);
        self.handle_requests(now);
    }

    /// Handles an input event read from the terminal.
    pub fn input_event(&mut self, event: InputEvent, now: Duration) {
        self.tui.input_event(event);
        self.handle_requests(now);
    }

    /// Stops the run, as Ctrl-C in the UI does.
    pub fn cancel(&mut self, now: Duration) {
        self.handle(Request::CancelRun, now);
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.tui.resize(cols, rows);
    }

    /// Advances the run to `now` and returns the bytes to write.
    pub fn frame(&mut self, now: Duration) -> Vec<u8> {
        self.tui.tick(now);
        if self.summary.is_none()
            && let Some(summary) = self.sim.advance(millis(now), &mut self.tui, &self.name)
        {
            self.finish(summary);
        }
        self.tui.render()
    }

    /// Whether the report has been printed and the UI is gone.
    #[must_use]
    pub const fn is_done(&self) -> bool {
        self.tui.is_done()
    }

    /// The exit code of `vp run` for this run.
    #[must_use]
    pub fn exit_code(&self) -> u8 {
        self.sim.exit_code()
    }

    /// See [`Tui::restore_sequence`].
    #[must_use]
    pub fn restore_sequence(&self) -> Vec<u8> {
        self.tui.restore_sequence()
    }

    /// What `vp run --last-details` prints for this run.
    #[must_use]
    pub fn last_details(&self) -> Vec<u8> {
        self.tui.last_details(self.summary.as_deref().unwrap_or_default().as_bytes())
    }

    fn handle_requests(&mut self, now: Duration) {
        for request in self.tui.take_requests() {
            self.handle(request, now);
        }
    }

    fn handle(&mut self, request: Request, now: Duration) {
        if let Some(summary) = self.sim.handle(request, millis(now), &mut self.tui, &self.name)
            && self.summary.is_none()
        {
            self.finish(summary);
        }
    }

    fn finish(&mut self, summary: Str) {
        self.tui.finish(summary.as_bytes());
        self.summary = Some(summary);
    }
}

pub fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}
