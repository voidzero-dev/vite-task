//! Plays scripted processes into the UI, the way `vp run` would report a real
//! run: dependencies, a concurrency limit, cache hits, fast-fail, and replies
//! to what the user types.

use vt_str::Str;
use vt_tui::{NodeId, Request, TaskState, Tone, Tui};

/// Where the simulator reports a run: the UI, or plain output.
pub trait Report {
    fn set_state(&mut self, task: NodeId, state: TaskState);
    fn set_status(&mut self, task: NodeId, status: Option<&str>, tone: Tone);
    fn task_output(&mut self, task: NodeId, data: &[u8]);
}

impl Report for Tui {
    fn set_state(&mut self, task: NodeId, state: TaskState) {
        Self::set_state(self, task, state);
    }

    fn set_status(&mut self, task: NodeId, status: Option<&str>, tone: Tone) {
        Self::set_status(self, task, status, tone);
    }

    fn task_output(&mut self, task: NodeId, data: &[u8]) {
        Self::task_output(self, task, data);
    }
}

/// Something a process does at a point in its script.
#[derive(Debug, Clone)]
pub enum Step {
    /// Writes to stdout.
    Out(Str),
    /// Reports a status, as a runner-aware tool would.
    Status(Option<Str>, Tone),
    /// Reports that a long-running task is ready.
    Ready,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    Succeed,
    Fail,
}

/// Replies to a line typed into a long-running process.
pub type Responder = fn(line: &str, clock: &str) -> Reply;

#[derive(Debug, Default)]
pub struct Reply {
    pub steps: Vec<Step>,
    pub end: Option<End>,
}

#[derive(Debug)]
pub struct Proc {
    pub id: NodeId,
    pub name: &'static str,
    /// Tasks this one depends on; while they run, its status names them.
    pub deps: Vec<usize>,
    /// Steps that come before this one, such as the previous `&&` command.
    pub after: Vec<usize>,
    /// Script entries at milliseconds since the process started.
    pub script: Vec<(u64, Step)>,
    /// When the process exits, and how. `None` for long-running processes.
    pub end: Option<(u64, End)>,
    /// Replayed from the cache: the script runs at once and the task never
    /// takes a concurrency slot.
    pub cache_hit: bool,
    pub responder: Option<Responder>,
    pub saved_ms: u64,
    run: Run,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Run {
    Pending,
    Running { start: u64, next: usize },
    Finished(Outcome),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Succeeded,
    CacheHit,
    Failed,
    Cancelled,
    NotRun,
}

impl Proc {
    pub const fn new(id: NodeId, name: &'static str) -> Self {
        Self {
            id,
            name,
            deps: Vec::new(),
            after: Vec::new(),
            script: Vec::new(),
            end: None,
            cache_hit: false,
            responder: None,
            saved_ms: 0,
            run: Run::Pending,
        }
    }
}

#[derive(Debug)]
pub struct Sim {
    pub procs: Vec<Proc>,
    pub concurrency: usize,
    /// The clock shown in log lines, as seconds since midnight at the start.
    pub clock_start: u64,
    done: bool,
    /// Whether the run was stopped by Ctrl-C.
    interrupted: bool,
}

impl Sim {
    pub const fn new(procs: Vec<Proc>, concurrency: usize) -> Self {
        Self {
            procs,
            concurrency,
            clock_start: 10 * 3600 + 42 * 60,
            done: false,
            interrupted: false,
        }
    }

    /// The wall clock at `now`, as the tools in the demo print it.
    pub fn clock(&self, now: u64) -> Str {
        let seconds = self.clock_start + now / 1000;
        let (hour, minute, second) = (seconds / 3600 % 24, seconds / 60 % 60, seconds % 60);
        let (hour, period) = if hour >= 12 { (hour - 12, "PM") } else { (hour, "AM") };
        vt_str::format!("{}:{minute:02}:{second:02} {period}", if hour == 0 { 12 } else { hour })
    }

    /// Advances to `now` milliseconds after the run started. Returns the
    /// summary once the run is over.
    pub fn advance(&mut self, now: u64, ui: &mut impl Report, run_name: &str) -> Option<Str> {
        if self.done {
            return None;
        }
        // Scripts of running processes.
        for index in 0..self.procs.len() {
            let Run::Running { start, mut next } = self.procs[index].run else { continue };
            let elapsed = now.saturating_sub(start);
            while next < self.procs[index].script.len()
                && self.procs[index].script[next].0 <= elapsed
            {
                let step = self.procs[index].script[next].1.clone();
                let clock = self.clock(now);
                apply(ui, self.procs[index].id, &step, &clock);
                next += 1;
            }
            self.procs[index].run = Run::Running { start, next };
            if let Some((at, end)) = self.procs[index].end
                && elapsed >= at
            {
                self.exit(index, end, ui);
            }
        }
        if self.procs.iter().any(|proc| proc.run == Run::Finished(Outcome::Failed)) {
            self.stop(ui, Outcome::Cancelled);
            return Some(self.summary(run_name, false));
        }
        self.start_ready(now, ui);
        if self.procs.iter().all(|proc| matches!(proc.run, Run::Finished(_))) {
            self.done = true;
            return Some(self.summary(run_name, false));
        }
        None
    }

    fn start_ready(&mut self, now: u64, ui: &mut impl Report) {
        let mut running = self
            .procs
            .iter()
            .filter(|proc| matches!(proc.run, Run::Running { .. }) && !proc.cache_hit)
            .count();
        for index in 0..self.procs.len() {
            if self.procs[index].run != Run::Pending {
                continue;
            }
            let finished = |deps: &[usize]| {
                deps.iter().all(|&dep| {
                    matches!(
                        self.procs[dep].run,
                        Run::Finished(Outcome::Succeeded | Outcome::CacheHit)
                    )
                })
            };
            if !finished(&self.procs[index].after) {
                continue;
            }
            let proc = &self.procs[index];
            if let Some(&blocking) = proc.deps.iter().find(|&&dep| !finished(&[dep])) {
                let name = self.procs[blocking].name;
                ui.set_state(proc.id, TaskState::Waiting);
                ui.set_status(proc.id, Some(&vt_str::format!("waiting for {name}")), Tone::Normal);
                continue;
            }
            if proc.cache_hit {
                let id = proc.id;
                let script: Vec<Step> = proc.script.iter().map(|(_, step)| step.clone()).collect();
                ui.set_status(id, None, Tone::Normal);
                let clock = self.clock(now);
                for step in &script {
                    apply(ui, id, step, &clock);
                }
                ui.set_state(id, TaskState::CacheHit);
                self.procs[index].run = Run::Finished(Outcome::CacheHit);
                continue;
            }
            if running >= self.concurrency {
                ui.set_state(proc.id, TaskState::Queued);
                ui.set_status(proc.id, Some("queued"), Tone::Normal);
                continue;
            }
            running += 1;
            ui.set_status(proc.id, None, Tone::Normal);
            ui.set_state(proc.id, TaskState::Running);
            self.procs[index].run = Run::Running { start: now, next: 0 };
        }
        // Run the steps due right at the start, such as a first status.
        for index in 0..self.procs.len() {
            if let Run::Running { start, next: 0 } = self.procs[index].run
                && start == now
            {
                let clock = self.clock(now);
                let mut next = 0;
                while next < self.procs[index].script.len() && self.procs[index].script[next].0 == 0
                {
                    let step = self.procs[index].script[next].1.clone();
                    apply(ui, self.procs[index].id, &step, &clock);
                    next += 1;
                }
                self.procs[index].run = Run::Running { start, next };
            }
        }
    }

    fn exit(&mut self, index: usize, end: End, ui: &mut impl Report) {
        let (outcome, state) = match end {
            End::Succeed => (Outcome::Succeeded, TaskState::Succeeded),
            End::Fail => (Outcome::Failed, TaskState::Failed),
        };
        self.procs[index].run = Run::Finished(outcome);
        ui.set_state(self.procs[index].id, state);
    }

    /// Stops everything still running or waiting.
    fn stop(&mut self, ui: &mut impl Report, running_outcome: Outcome) {
        self.done = true;
        for proc in &mut self.procs {
            match proc.run {
                Run::Running { .. } => {
                    proc.run = Run::Finished(running_outcome);
                    ui.set_status(proc.id, None, Tone::Normal);
                    ui.set_state(proc.id, TaskState::Cancelled);
                }
                Run::Pending => {
                    proc.run = Run::Finished(Outcome::NotRun);
                    ui.set_status(proc.id, None, Tone::Normal);
                    ui.set_state(proc.id, TaskState::NotRun);
                }
                Run::Finished(_) => {}
            }
        }
    }

    /// Acts on what the user asked for in the UI. Returns the summary if the
    /// run stopped.
    pub fn handle(
        &mut self,
        request: Request,
        now: u64,
        ui: &mut impl Report,
        run_name: &str,
    ) -> Option<Str> {
        match request {
            Request::CancelRun => {
                if self.done {
                    return None;
                }
                self.interrupted = true;
                self.stop(ui, Outcome::Cancelled);
                return Some(self.summary(run_name, true));
            }
            Request::Input { task, data } => {
                let index = self.procs.iter().position(|proc| proc.id == task)?;
                let responder = self.procs[index].responder?;
                let line = std::str::from_utf8(&data).unwrap_or_default().trim();
                let clock = self.clock(now);
                let reply = responder(line, &clock);
                for step in &reply.steps {
                    apply(ui, task, step, &clock);
                }
                if let Some(end) = reply.end {
                    self.exit(index, end, ui);
                }
            }
            Request::CloseInput { task } | Request::Interrupt { task } => {
                let index = self.procs.iter().position(|proc| proc.id == task)?;
                if matches!(self.procs[index].run, Run::Running { .. }) {
                    let message: &[u8] = if matches!(request, Request::CloseInput { .. }) {
                        b"\x1b[2mstdin closed, shutting down\x1b[22m\n"
                    } else {
                        b"\x1b[2m^C received, shutting down\x1b[22m\n"
                    };
                    ui.task_output(task, message);
                    self.procs[index].run = Run::Finished(Outcome::Cancelled);
                    ui.set_status(task, None, Tone::Normal);
                    ui.set_state(task, TaskState::Cancelled);
                }
            }
        }
        if !self.done && self.procs.iter().all(|proc| matches!(proc.run, Run::Finished(_))) {
            self.done = true;
            return Some(self.summary(run_name, false));
        }
        None
    }

    /// The exit code of `vp run` for the run so far: 130 if it was stopped by
    /// Ctrl-C, 1 if a task failed, 0 otherwise.
    pub fn exit_code(&self) -> u8 {
        if self.interrupted {
            return 130;
        }
        u8::from(self.procs.iter().any(|proc| proc.run == Run::Finished(Outcome::Failed)))
    }

    fn summary(&self, run_name: &str, interrupted: bool) -> Str {
        let cacheable: Vec<&Proc> = self.procs.iter().filter(|proc| proc.end.is_some()).collect();
        let hits =
            cacheable.iter().filter(|proc| proc.run == Run::Finished(Outcome::CacheHit)).count();
        let total = cacheable.len();
        let failed =
            self.procs.iter().filter(|proc| proc.run == Run::Finished(Outcome::Failed)).count();
        let successful = self
            .procs
            .iter()
            .filter(|proc| {
                matches!(proc.run, Run::Finished(Outcome::Succeeded | Outcome::CacheHit))
            })
            .count();
        let saved: u64 = cacheable
            .iter()
            .filter(|proc| proc.run == Run::Finished(Outcome::CacheHit))
            .map(|proc| proc.saved_ms)
            .sum();
        let details = vt_str::format!(
            "\x1b[2m(Run `vp run --last-details {run_name}` for full details)\x1b[22m"
        );
        if interrupted {
            return vt_str::format!(
                "\r\n\x1b[2m---\x1b[22m\r\nvp run: stopped by Ctrl-C. {details}\r\n"
            );
        }
        let rate = (hits * 100).checked_div(total).unwrap_or(0);
        let mut line = vt_str::format!("vp run: {hits}/{total} cache hit ({rate}%), ");
        if failed > 0 {
            line.push_str(&vt_str::format!("\x1b[31m{failed} failed\x1b[39m"));
        } else {
            line.push_str(&vt_str::format!("{successful}/{} successful", self.procs.len()));
        }
        if saved > 0 {
            line.push_str(&vt_str::format!(", {}.{}s saved", saved / 1000, saved % 1000 / 100));
        }
        vt_str::format!("\r\n\x1b[2m---\x1b[22m\r\n{line}. {details}\r\n")
    }
}

fn apply(ui: &mut impl Report, id: NodeId, step: &Step, clock: &str) {
    match step {
        Step::Out(text) => {
            let text = text.as_str().replace_clock(clock);
            ui.task_output(id, text.as_bytes());
        }
        Step::Status(status, tone) => ui.set_status(id, status.as_deref(), *tone),
        Step::Ready => ui.set_state(id, TaskState::Ready),
    }
}

const CLOCK: &str = "%clock%";

/// Script output can say `%clock%` where the wall clock goes.
pub trait ReplaceClock {
    fn replace_clock(&self, clock: &str) -> Str;
}

impl ReplaceClock for str {
    fn replace_clock(&self, clock: &str) -> Str {
        let mut out = Str::with_capacity(self.len());
        let mut rest = self;
        while let Some(index) = rest.find(CLOCK) {
            out.push_str(&rest[..index]);
            out.push_str(clock);
            rest = &rest[index + CLOCK.len()..];
        }
        out.push_str(rest);
        out
    }
}
