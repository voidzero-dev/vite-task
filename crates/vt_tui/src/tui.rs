//! The state machine the caller drives.

use std::time::Duration;

use base64::Engine as _;
use terminput::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use vt_str::Str;

use crate::{
    ansi::{
        DISABLE_MOUSE, ENABLE_MOUSE, ENTER_ALT_SCREEN, HIDE_CURSOR, InlineRenderer,
        LEAVE_ALT_SCREEN, SHOW_CURSOR, SYNC_BEGIN, SYNC_END, ScreenRenderer, strip_styles,
    },
    emulator::Emulator,
    input::InputParser,
    tree::{NodeId, TaskState, TaskTree, Tone},
};

/// How often agent mode draws the tree at most.
const AGENT_FRAME_INTERVAL: Duration = Duration::from_secs(2);

/// How the UI behaves. Built with [`Options::new`].
#[derive(Debug, Clone)]
pub struct Options {
    pub cols: u16,
    pub rows: u16,
    /// The name `vp run --last-details` takes to show this run again.
    pub run_name: Str,
    /// Animate the icon of running tasks. Off for AI agents, so a frame is
    /// only drawn when something changes.
    pub animate: bool,
    /// Show the status tools report while their task runs, and how many of
    /// a group's tasks are done. Off for AI agents; a task's status is still
    /// shown once it's ready or finished.
    pub live_status: bool,
    /// Colors, the key hints, the selection column, aligned statuses and the
    /// rules in the report: what helps a person read the terminal. Off for AI
    /// agents, which read every byte as tokens. The task view keeps them.
    pub decorations: bool,
    /// The least time between two frames of the tree, so frames that a newer
    /// one replaces before anyone reads them are skipped. Input still redraws
    /// at once.
    pub frame_interval: Duration,
    /// Nothing is drawn before this much time has passed, so a run that's
    /// over quickly, such as one with only cache hits, prints just its report.
    pub first_frame_delay: Duration,
}

impl Options {
    #[must_use]
    pub fn new(cols: u16, rows: u16, run_name: impl Into<Str>) -> Self {
        Self {
            cols,
            rows,
            run_name: run_name.into(),
            animate: true,
            live_status: true,
            decorations: true,
            frame_interval: Duration::ZERO,
            first_frame_delay: Duration::from_millis(150),
        }
    }

    /// The settings for an AI agent driving the terminal.
    #[must_use]
    pub const fn for_agent(mut self) -> Self {
        self.animate = false;
        self.live_status = false;
        self.decorations = false;
        self.frame_interval = AGENT_FRAME_INTERVAL;
        self
    }
}

/// Something the UI asks the caller to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Write `data` to the task's stdin. Sent when the user types a line in
    /// the task view.
    Input { task: NodeId, data: Vec<u8> },
    /// Close the task's stdin (Ctrl-D in the task view).
    CloseInput { task: NodeId },
    /// Send the task an interrupt (Ctrl-C in the task view).
    Interrupt { task: NodeId },
    /// Stop the run (Ctrl-C).
    CancelRun,
}

/// A row of the tree. Rows folded together keep the first node's id, so the
/// selection can stay on them while more tasks finish.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKey {
    Node(NodeId),
    /// Siblings that finished cleanly, folded into one row.
    Done {
        parent: Option<NodeId>,
        first: NodeId,
        count: usize,
    },
    /// Rows above or below the part of the tree that fits on screen.
    More {
        above: bool,
        count: usize,
    },
}

impl RowKey {
    pub(crate) fn same_row(self, other: Self) -> bool {
        match (self, other) {
            (Self::Node(a), Self::Node(b)) => a == b,
            (Self::Done { parent: a, first: c, .. }, Self::Done { parent: b, first: d, .. }) => {
                a == b && c == d
            }
            _ => false,
        }
    }
}

#[derive(Debug)]
pub enum Mode {
    Browse,
    /// Ctrl-F: typing goes to the search query instead of the task.
    Search {
        query: Str,
    },
}

#[derive(Debug)]
pub struct TaskView {
    pub task: NodeId,
    pub mode: Mode,
    /// What the user has typed for the task since the last Enter.
    pub line: Str,
    /// A short notice in the bar, shown until the given time.
    pub notice: Option<(Str, Duration)>,
    /// Whether the mouse moved with the button held since it was pressed.
    pub dragged: bool,
}

#[derive(Debug)]
pub enum View {
    Tree,
    Task(TaskView),
}

#[derive(Debug)]
enum Phase {
    Running,
    /// The run is over, but the user is still in the task view.
    Finishing {
        summary: Vec<u8>,
    },
    Done,
}

/// The terminal UI of one run. It does no I/O: feed it the run's events and
/// the terminal's input, and write what [`Tui::render`] returns to the terminal.
#[derive(Debug)]
#[expect(clippy::struct_excessive_bools, reason = "independent flags of the UI's state")]
pub struct Tui {
    pub(crate) tree: TaskTree,
    pub(crate) options: Options,
    pub(crate) now: Duration,
    pub(crate) view: View,
    /// `None` until the user moves through the tree.
    pub(crate) selected: Option<RowKey>,
    pub(crate) scroll_top: usize,
    /// Whether finished siblings at the top level are listed one by one.
    pub(crate) show_done_roots: bool,
    pub(crate) emulators: Vec<Option<Emulator>>,
    inline: InlineRenderer,
    screen: ScreenRenderer,
    out: Vec<u8>,
    requests: Vec<Request>,
    dirty: bool,
    started: bool,
    phase: Phase,
    parser: InputParser,
    pub(crate) spinner: usize,
    /// When the last frame was drawn, for `Options::frame_interval`.
    last_frame: Option<Duration>,
    /// Set by input, so its effect is drawn without waiting for the interval.
    input_pending: bool,
}

impl Tui {
    #[must_use]
    pub fn new(tree: TaskTree, options: Options) -> Self {
        let emulators = tree
            .ids()
            .map(|id| {
                (!tree.node(id).is_group())
                    .then(|| Emulator::new(options.cols, options.rows.saturating_sub(1)))
            })
            .collect();
        Self {
            tree,
            options,
            now: Duration::ZERO,
            view: View::Tree,
            selected: None,
            scroll_top: 0,
            show_done_roots: false,
            emulators,
            inline: InlineRenderer::default(),
            screen: ScreenRenderer::default(),
            out: Vec::new(),
            requests: Vec::new(),
            dirty: true,
            started: false,
            phase: Phase::Running,
            parser: InputParser::default(),
            spinner: 0,
            last_frame: None,
            input_pending: false,
        }
    }

    /// Advances the clock. `now` is the time since the run started.
    pub fn tick(&mut self, now: Duration) {
        self.now = now;
        if self.options.animate {
            let frame = usize::try_from(now.as_millis() / 100).unwrap_or(0) % SPINNER.len();
            if frame != self.spinner {
                self.spinner = frame;
                if self.any_running() {
                    self.dirty = true;
                }
            }
        }
        if let View::Task(view) = &mut self.view
            && view.notice.as_ref().is_some_and(|(_, until)| *until <= now)
        {
            view.notice = None;
            self.dirty = true;
        }
    }

    pub fn set_state(&mut self, task: NodeId, state: TaskState) {
        let now = self.now;
        let node = self.tree.node_mut(task);
        if node.state == state {
            return;
        }
        if state.is_started() && state != TaskState::NotRun && node.started_at.is_none() {
            node.started_at = Some(now);
        }
        if state.is_finished() {
            node.finished_at = Some(now);
        }
        node.state = state;
        self.dirty = true;
    }

    /// Sets the status shown at the end of the task's row, or clears it.
    pub fn set_status(&mut self, task: NodeId, status: Option<&str>, tone: Tone) {
        let node = self.tree.node_mut(task);
        if node.status.as_deref() == status && node.tone == tone {
            return;
        }
        node.status = status.map(Str::from);
        node.tone = tone;
        if self.options.live_status || self.tree.node(task).state != TaskState::Running {
            self.dirty = true;
        }
    }

    /// Adds output the task wrote to its stdout or stderr.
    pub fn task_output(&mut self, task: NodeId, data: &[u8]) {
        if let Some(Some(emulator)) = self.emulators.get_mut(task.index()) {
            emulator.feed(data);
            if matches!(&self.view, View::Task(view) if view.task == task) {
                self.dirty = true;
            }
        }
    }

    /// Handles bytes read from the terminal.
    pub fn input(&mut self, data: &[u8]) {
        let mut events = Vec::new();
        self.parser.feed(data, |event| events.push(event));
        for event in events {
            self.input_event(event);
        }
    }

    /// Handles an input event the caller parsed itself, such as one read with
    /// crossterm where the terminal's input isn't a byte stream (Windows).
    pub fn input_event(&mut self, event: Event) {
        if matches!(self.phase, Phase::Done) {
            return;
        }
        self.started = true;
        self.input_pending = true;
        self.handle_event(event);
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        if (cols, rows) == (self.options.cols, self.options.rows) {
            return;
        }
        self.options.cols = cols;
        self.options.rows = rows;
        for emulator in self.emulators.iter_mut().flatten() {
            emulator.resize(cols, rows.saturating_sub(1));
        }
        match self.view {
            View::Tree => self.inline.clear(&mut self.out),
            View::Task(_) => {
                self.out.extend_from_slice(b"\x1b[2J");
                self.screen.invalidate();
            }
        }
        self.dirty = true;
    }

    /// Takes the requests made since the last call.
    pub fn take_requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.requests)
    }

    /// Ends the run. The last frame of the tree stays on screen, followed by
    /// the output of the tasks that failed or warned, and then `summary`.
    ///
    /// If the user is in the task view, this waits until they leave it.
    pub fn finish(&mut self, summary: impl Into<Vec<u8>>) {
        if !matches!(self.phase, Phase::Running) {
            return;
        }
        self.phase = Phase::Finishing { summary: summary.into() };
        if matches!(self.view, View::Tree) {
            self.finalize();
        } else {
            self.dirty = true;
        }
    }

    /// Whether the report has been printed and the UI is gone.
    #[must_use]
    pub const fn is_done(&self) -> bool {
        matches!(self.phase, Phase::Done)
    }

    /// The bytes that hand the terminal back if the program stops before the
    /// UI is done, such as on a panic: they leave the task view and show the
    /// cursor below the tree. They assume everything [`Tui::render`] returned
    /// has been written.
    #[must_use]
    pub fn restore_sequence(&self) -> Vec<u8> {
        if self.is_done() {
            return Vec::new();
        }
        let mut out = SYNC_END.to_vec();
        if matches!(self.view, View::Task(_)) {
            out.extend_from_slice(DISABLE_MOUSE);
            out.extend_from_slice(LEAVE_ALT_SCREEN);
        }
        if self.inline.is_drawn() {
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(SHOW_CURSOR);
        out
    }

    pub(crate) const fn is_finishing(&self) -> bool {
        matches!(self.phase, Phase::Finishing { .. })
    }

    /// Everything `vp run --last-details` prints: every task's output in tree
    /// order, then `summary`.
    #[must_use]
    pub fn last_details(&self, summary: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        self.write_report(&mut out, true);
        out.extend_from_slice(summary);
        out
    }

    /// The bytes to write to the terminal since the last call.
    pub fn render(&mut self) -> Vec<u8> {
        if matches!(self.phase, Phase::Done) {
            return std::mem::take(&mut self.out);
        }
        if !self.started {
            if self.now < self.options.first_frame_delay {
                return std::mem::take(&mut self.out);
            }
            self.started = true;
        }
        let throttled = !self.input_pending
            && matches!(self.view, View::Tree)
            && self.last_frame.is_some_and(|last| self.now < last + self.options.frame_interval);
        if self.dirty && !throttled {
            self.dirty = false;
            self.input_pending = false;
            self.last_frame = Some(self.now);
            let mut frame = Vec::new();
            match &self.view {
                View::Tree => {
                    if !self.inline.is_drawn() {
                        frame.extend_from_slice(HIDE_CURSOR);
                    }
                    let rows = self.tree_rows(true);
                    self.inline.draw(rows, &mut frame);
                }
                View::Task(_) => {
                    let rows = self.task_rows();
                    self.screen.draw(rows, &mut frame);
                }
            }
            if !frame.is_empty() {
                self.write_frame(&frame);
            }
        }
        std::mem::take(&mut self.out)
    }

    fn finalize(&mut self) {
        let Phase::Finishing { summary } = std::mem::replace(&mut self.phase, Phase::Running)
        else {
            return;
        };
        if self.started {
            let mut frame = Vec::new();
            if !self.inline.is_drawn() {
                frame.extend_from_slice(HIDE_CURSOR);
            }
            let rows = self.tree_rows(false);
            self.inline.draw(rows, &mut frame);
            self.write_frame(&frame);
            self.out.extend_from_slice(b"\r\n");
            self.inline.detach();
        }
        let mut out = Vec::new();
        self.write_report(&mut out, false);
        if self.options.decorations {
            out.extend_from_slice(&summary);
        } else {
            out.extend_from_slice(&strip_styles(&summary));
        }
        out.extend_from_slice(SHOW_CURSOR);
        self.out.extend_from_slice(&out);
        self.phase = Phase::Done;
    }

    /// Writes a frame, as one synchronized update when decorated.
    fn write_frame(&mut self, frame: &[u8]) {
        if self.options.decorations {
            self.out.extend_from_slice(SYNC_BEGIN);
            self.out.extend_from_slice(frame);
            self.out.extend_from_slice(SYNC_END);
        } else {
            self.out.extend_from_slice(frame);
        }
    }

    pub(crate) fn any_running(&self) -> bool {
        self.tree.ids().any(|id| {
            let node = self.tree.node(id);
            !node.is_group() && node.state == TaskState::Running
        })
    }

    pub(crate) fn is_running(&self, task: NodeId) -> bool {
        let state = self.tree.node(task).state;
        state.is_started() && !state.is_finished()
    }

    pub(crate) fn accepts_input(&self, task: NodeId) -> bool {
        let node = self.tree.node(task);
        node.spec.accepts_input && !node.state.is_finished() && node.state.is_started()
    }

    pub(crate) fn notice(&mut self, text: &str) {
        let until = self.now + Duration::from_secs(3);
        if let View::Task(view) = &mut self.view {
            view.notice = Some((Str::from(text), until));
        }
        self.dirty = true;
    }

    pub(crate) fn request(&mut self, request: Request) {
        self.requests.push(request);
    }

    pub(crate) fn emulator_mut(&mut self, task: NodeId) -> Option<&mut Emulator> {
        self.emulators.get_mut(task.index()).and_then(Option::as_mut)
    }

    /// Copies text to the clipboard of the terminal the UI runs in (OSC 52).
    pub(crate) fn copy_to_clipboard(&mut self, text: &str) {
        self.out.extend_from_slice(b"\x1b]52;c;");
        self.out
            .extend_from_slice(base64::engine::general_purpose::STANDARD.encode(text).as_bytes());
        self.out.push(0x07);
    }

    pub(crate) fn open_task(&mut self, task: NodeId) {
        if !matches!(self.view, View::Task(_)) {
            self.out.extend_from_slice(ENTER_ALT_SCREEN);
            self.out.extend_from_slice(ENABLE_MOUSE);
            self.out.extend_from_slice(b"\x1b[2J");
            self.screen.invalidate();
        }
        self.view = View::Task(TaskView {
            task,
            mode: Mode::Browse,
            line: Str::default(),
            notice: None,
            dragged: false,
        });
        self.dirty = true;
    }

    pub(crate) fn close_task(&mut self) {
        if let View::Task(view) = &self.view
            && let Some(emulator) =
                self.emulators.get_mut(view.task.index()).and_then(Option::as_mut)
        {
            emulator.clear_selection();
        }
        self.out.extend_from_slice(DISABLE_MOUSE);
        self.out.extend_from_slice(LEAVE_ALT_SCREEN);
        self.view = View::Tree;
        self.dirty = true;
        if self.is_finishing() {
            self.finalize();
        }
    }

    pub(crate) const fn viewed_task(&self) -> Option<NodeId> {
        match &self.view {
            View::Task(view) => Some(view.task),
            View::Tree => None,
        }
    }

    pub(crate) const fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    fn handle_event(&mut self, event: Event) {
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => match self.view {
                View::Tree if is_ctrl(key, 'c') => self.request(Request::CancelRun),
                View::Tree => self.tree_key(key),
                View::Task(_) => self.task_key(key),
            },
            Event::Mouse(mouse) => {
                if matches!(self.view, View::Task(_)) {
                    self.task_mouse(mouse);
                }
            }
            Event::Paste(text) => {
                let Some(task) = self.viewed_task() else { return };
                let accepts_input = self.accepts_input(task);
                if let View::Task(view) = &mut self.view {
                    let field = match &mut view.mode {
                        Mode::Search { query } => query,
                        Mode::Browse if accepts_input => &mut view.line,
                        Mode::Browse => return,
                    };
                    for c in text.chars() {
                        field.push(if c == '\n' || c == '\r' { ' ' } else { c });
                    }
                    self.dirty = true;
                }
            }
            Event::Resize { cols, rows } => {
                self.resize(
                    u16::try_from(cols).unwrap_or(u16::MAX),
                    u16::try_from(rows).unwrap_or(u16::MAX),
                );
            }
            _ => {}
        }
    }
}

pub fn is_ctrl(key: KeyEvent, c: char) -> bool {
    key.modifiers.contains(KeyModifiers::CTRL) && key.code == KeyCode::Char(c)
}

pub const SPINNER: [&str; 8] = ["⣾", "⣽", "⣻", "⢿", "⡿", "⣟", "⣯", "⣷"];
