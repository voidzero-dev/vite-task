//! A run printed without the UI, for when stdout isn't a terminal: each line a
//! task writes, prefixed with the task's label as `vp run` labels it, and then
//! the summary.

use std::time::Duration;

use rustc_hash::FxHashMap;
use vt_str::Str;
use vt_tui::{NodeId, Request, TaskState, TaskTree, Tone};

use crate::{
    player::millis,
    scenarios::Scenario,
    sim::{Report, Sim},
};

/// A scenario's run as plain output. The bytes keep the colors the tasks
/// wrote; the caller strips them if the output doesn't take colors.
#[derive(Debug)]
pub struct Plain {
    name: Str,
    sim: Sim,
    printer: Printer,
    done: bool,
}

impl Plain {
    #[must_use]
    pub fn new(scenario: Scenario, name: impl Into<Str>) -> Self {
        Self {
            name: name.into(),
            sim: scenario.sim,
            printer: Printer {
                tree: scenario.tree,
                partial: FxHashMap::default(),
                out: Vec::new(),
            },
            done: false,
        }
    }

    /// Advances the run to `now`, the time since it started, and returns the
    /// bytes to write.
    pub fn frame(&mut self, now: Duration) -> Vec<u8> {
        if let Some(summary) = self.sim.advance(millis(now), &mut self.printer, &self.name) {
            self.finish(&summary);
        }
        std::mem::take(&mut self.printer.out)
    }

    /// Stops the run, as Ctrl-C does.
    pub fn cancel(&mut self, now: Duration) {
        if let Some(summary) =
            self.sim.handle(Request::CancelRun, millis(now), &mut self.printer, &self.name)
        {
            self.finish(&summary);
        }
    }

    #[must_use]
    pub const fn is_done(&self) -> bool {
        self.done
    }

    /// The exit code of `vp run` for this run.
    #[must_use]
    pub fn exit_code(&self) -> u8 {
        self.sim.exit_code()
    }

    fn finish(&mut self, summary: &str) {
        let printer = &mut self.printer;
        let mut partial: Vec<(NodeId, Vec<u8>)> = printer.partial.drain().collect();
        partial.sort_by_key(|(task, _)| *task);
        for (task, line) in partial {
            printer.write_line(task, &line);
        }
        printer.out.extend(summary.bytes().filter(|&byte| byte != b'\r'));
        self.done = true;
    }
}

#[derive(Debug)]
struct Printer {
    tree: TaskTree,
    /// What each task wrote after its last complete line.
    partial: FxHashMap<NodeId, Vec<u8>>,
    out: Vec<u8>,
}

impl Printer {
    /// The task a process belongs to, `pkg#task`: the process itself, or the
    /// task it's a step of.
    fn label(&self, process: NodeId) -> &str {
        let mut node = Some(process);
        while let Some(id) = node {
            let label = &self.tree.spec(id).label;
            if label.contains('#') {
                return label;
            }
            node = self.tree.parent(id);
        }
        &self.tree.spec(process).label
    }

    fn write_line(&mut self, task: NodeId, line: &[u8]) {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let prefix = vt_str::format!("\x1b[90m[{}]\x1b[39m ", self.label(task));
        self.out.extend_from_slice(prefix.as_bytes());
        self.out.extend_from_slice(line);
        self.out.push(b'\n');
    }
}

impl Report for Printer {
    fn set_state(&mut self, _task: NodeId, _state: TaskState) {}

    fn set_status(&mut self, _task: NodeId, _status: Option<&str>, _tone: Tone) {}

    fn task_output(&mut self, task: NodeId, data: &[u8]) {
        let mut rest = data;
        while let Some(end) = rest.iter().position(|&byte| byte == b'\n') {
            let mut line = self.partial.remove(&task).unwrap_or_default();
            line.extend_from_slice(&rest[..end]);
            self.write_line(task, &line);
            rest = &rest[end + 1..];
        }
        if !rest.is_empty() {
            self.partial.entry(task).or_default().extend_from_slice(rest);
        }
    }
}
