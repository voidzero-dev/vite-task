//! One task's output in the alternate screen, with a bar at the bottom.

use alacritty_terminal::{grid::Scroll, index::Direction};
use ratatui_core::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
};
use terminput::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind, ScrollDirection,
};
use unicode_width::UnicodeWidthStr as _;
use vt_str::Str;

use crate::{
    ansi::encode_rows,
    tree::NodeId,
    tree_view::{DIM, put},
    tui::{Mode, Request, Tui, View, is_ctrl},
};

const BAR: Style = Style::new().bg(Color::Indexed(236)).fg(Color::Gray);
const BAR_KEY: Style =
    Style::new().bg(Color::Indexed(236)).fg(Color::White).add_modifier(Modifier::BOLD);
const BAR_DIM: Style = Style::new().bg(Color::Indexed(236)).fg(Color::Indexed(245));
const BAR_MATCH: Style = Style::new().bg(Color::Indexed(236)).fg(Color::Indexed(214));
/// Typed input, styled like the echo it becomes once sent.
const TYPED: Style = Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD);
const WHEEL_LINES: i32 = 3;

impl Tui {
    const fn task_view(&self) -> Option<&crate::tui::TaskView> {
        match &self.view {
            View::Task(view) => Some(view),
            View::Tree => None,
        }
    }

    const fn task_view_mut(&mut self) -> Option<&mut crate::tui::TaskView> {
        match &mut self.view {
            View::Task(view) => Some(view),
            View::Tree => None,
        }
    }

    pub(crate) fn task_rows(&mut self) -> Vec<Vec<u8>> {
        let width = self.options.cols;
        let height = self.options.rows.max(2);
        let mut buffer = Buffer::empty(Rect::new(0, 0, width, height));
        let Some(task) = self.viewed_task() else { return Vec::new() };
        let accepts_input = self.accepts_input(task);
        let line = self.task_view().map(|view| match view.mode {
            Mode::Browse => Some(view.line.clone()),
            Mode::Search { .. } => None,
        });
        if let Some(emulator) = self.emulator_mut(task) {
            if emulator.has_output() {
                emulator.render(&mut buffer, Rect::new(0, 0, width, height - 1));
            } else {
                put(&mut buffer, 0, 0, "No output yet.", DIM, width);
            }
            // What's typed shows at the task's cursor, the way a terminal
            // echoes it, until Enter sends it.
            if accepts_input
                && let Some(Some(line)) = line
                && let Some((col, row)) = emulator.cursor()
                && row < height - 1
            {
                let x = put(&mut buffer, col, row, &line, TYPED, width);
                put(&mut buffer, x, row, " ", Style::new().bg(Color::Gray), width);
            }
        }
        self.draw_bar(&mut buffer, task, height - 1);
        // The task view keeps its colors even for agents: it only opens on a key.
        encode_rows(&buffer, true)
    }

    fn draw_bar(&mut self, buffer: &mut Buffer, task: NodeId, y: u16) {
        let width = self.options.cols;
        buffer.set_style(Rect::new(0, y, width, 1), BAR);
        let accepts_input = self.accepts_input(task);
        let running = self.is_running(task);
        let finishing = self.is_finishing();
        let (scrolled_back, search) = self
            .emulator_mut(task)
            .map_or((0, None), |emulator| (emulator.scrolled_back(), emulator.search_position()));
        let Some(view) = self.task_view() else { return };

        // Right side: what the keys do now.
        let mut hints: Vec<(&str, &str)> = Vec::new();
        let mut right: Option<Str> = None;
        match &view.mode {
            Mode::Browse => {
                if let Some((notice, _)) = &view.notice {
                    right = Some(notice.clone());
                } else if finishing {
                    right = Some(Str::from("the run is over · esc for the report"));
                } else {
                    hints.push(("esc", "tree"));
                    hints.push(("ctrl-f", "search"));
                    if running {
                        hints.push(("ctrl-c", "interrupt"));
                    }
                    if accepts_input && view.line.is_empty() {
                        hints.push(("type", "to send input"));
                    }
                }
            }
            Mode::Search { .. } => {
                hints.push(("↑↓", "older newer"));
                hints.push(("enter", "done"));
                hints.push(("esc", "clear"));
            }
        }
        // Left side first: the task, or the search field. The hints on the
        // right make room for it.
        let left_end = self.draw_bar_left(buffer, task, y, search, scrolled_back);
        let available = width.saturating_sub(left_end + 2);
        if let Some(text) = right {
            let text_width = u16::try_from(text.width()).unwrap_or(u16::MAX);
            let x = width.saturating_sub(text_width + 1).max(left_end + 2);
            put(buffer, x, y, &text, Style::new().bg(Color::Indexed(236)).fg(Color::Yellow), width);
            return;
        }
        let hint_width = |(key, description): &(&str, &str)| {
            u16::try_from(key.width() + description.width() + 3).unwrap_or(0)
        };
        while hints.iter().map(hint_width).sum::<u16>() > available {
            hints.pop();
        }
        let mut x = width.saturating_sub(hints.iter().map(hint_width).sum::<u16>());
        for (key, description) in hints {
            x = put(buffer, x, y, key, BAR_KEY, width);
            x = put(buffer, x + 1, y, description, BAR_DIM, width);
            x += 2;
        }
    }

    /// Draws the left part of the bar and returns where it ends.
    fn draw_bar_left(
        &self,
        buffer: &mut Buffer,
        task: NodeId,
        y: u16,
        search: Option<(usize, usize)>,
        scrolled_back: usize,
    ) -> u16 {
        let width = self.options.cols;
        let Some(view) = self.task_view() else { return 0 };
        let mut x = 1;
        let match_text = |position: usize, count: usize| {
            if count == 0 {
                Str::from("no matches")
            } else {
                vt_str::format!("match {position}/{count}")
            }
        };
        match &view.mode {
            Mode::Browse => {
                let effective = self.tree.effective(task);
                let (icon, icon_style) = self.icon(effective);
                x = put(buffer, x, y, icon, icon_style.bg(Color::Indexed(236)), width);
                x = put(buffer, x + 1, y, &self.title(task), BAR_KEY, width);
                for text in self.status_spans(task).iter().map(|(text, _)| text) {
                    x = put(buffer, x, y, "  ", BAR, width);
                    x = put(buffer, x, y, text, BAR_DIM, width);
                }
                if let Some((position, count)) = search {
                    x = put(buffer, x, y, "  ", BAR, width);
                    x = put(buffer, x, y, &match_text(position, count), BAR_MATCH, width);
                }
                if scrolled_back > 0 {
                    let text = vt_str::format!("↑ {scrolled_back} lines · end to follow");
                    x = put(buffer, x, y, "  ", BAR, width);
                    x = put(
                        buffer,
                        x,
                        y,
                        &text,
                        Style::new().bg(Color::Indexed(236)).fg(Color::Cyan),
                        width,
                    );
                }
            }
            Mode::Search { query } => {
                x = put(buffer, x, y, "search › ", BAR_KEY, width);
                x = put(
                    buffer,
                    x,
                    y,
                    query,
                    Style::new().bg(Color::Indexed(236)).fg(Color::White),
                    width,
                );
                x = put(buffer, x, y, " ", Style::new().bg(Color::Gray), width);
                if let Some((position, count)) = search.filter(|_| !query.is_empty()) {
                    x = put(buffer, x + 2, y, &match_text(position, count), BAR_MATCH, width);
                }
            }
        }
        x
    }

    pub(crate) fn task_key(&mut self, key: KeyEvent) {
        let Some(view) = self.task_view() else { return };
        let task = view.task;
        match &view.mode {
            Mode::Browse => self.browse_key(task, key),
            Mode::Search { .. } => self.search_key(task, key),
        }
    }

    /// Keys while reading a task's output. Anything typed goes to the task;
    /// the view itself only answers to keys that aren't typed.
    fn browse_key(&mut self, task: NodeId, key: KeyEvent) {
        let scroll = |tui: &mut Self, scroll: Scroll| {
            if let Some(emulator) = tui.emulator_mut(task) {
                emulator.scroll(scroll);
            }
            tui.mark_dirty();
        };
        if is_ctrl(key, 'c') {
            // Like a terminal, Ctrl-C goes to the task; once it has ended,
            // there's nothing to interrupt, so it stops the run instead.
            if self.is_running(task) {
                self.request(Request::Interrupt { task });
                self.notice("sent an interrupt");
            } else {
                self.request(Request::CancelRun);
            }
            return;
        }
        if is_ctrl(key, 'f') {
            let query = self
                .emulator_mut(task)
                .and_then(|emulator| emulator.search_pattern().map(Str::from))
                .unwrap_or_default();
            if let Some(view) = self.task_view_mut() {
                view.mode = Mode::Search { query };
            }
            self.mark_dirty();
            return;
        }
        if is_ctrl(key, 'd') {
            if self.accepts_input(task) {
                self.request(Request::CloseInput { task });
                self.notice("ended the task's input");
            } else {
                self.no_input_notice(task);
            }
            return;
        }
        match key.code {
            KeyCode::Esc => {
                let searching = self
                    .emulator_mut(task)
                    .is_some_and(|emulator| emulator.search_pattern().is_some());
                if let Some(emulator) = self.emulator_mut(task).filter(|_| searching) {
                    emulator.clear_search();
                    self.mark_dirty();
                } else {
                    self.close_task();
                }
            }
            KeyCode::Up => scroll(self, Scroll::Delta(1)),
            KeyCode::Down => scroll(self, Scroll::Delta(-1)),
            KeyCode::PageUp => scroll(self, Scroll::PageUp),
            KeyCode::PageDown => scroll(self, Scroll::PageDown),
            KeyCode::Home => scroll(self, Scroll::Top),
            KeyCode::End => scroll(self, Scroll::Bottom),
            KeyCode::Enter => self.send_line(task),
            KeyCode::Backspace => {
                if let Some(view) = self.task_view_mut() {
                    view.line.pop();
                }
                self.mark_dirty();
            }
            KeyCode::Char(c)
                if !key.modifiers.intersects(KeyModifiers::CTRL | KeyModifiers::ALT) =>
            {
                if !self.accepts_input(task) {
                    self.no_input_notice(task);
                    return;
                }
                if let Some(view) = self.task_view_mut() {
                    view.line.push(c);
                }
                // Typing brings the view back to the newest output.
                scroll(self, Scroll::Bottom);
            }
            _ => {}
        }
    }

    fn send_line(&mut self, task: NodeId) {
        if !self.accepts_input(task) {
            self.no_input_notice(task);
            return;
        }
        let Some(view) = self.task_view_mut() else { return };
        let sent = std::mem::take(&mut view.line);
        let mut data = sent.as_bytes().to_vec();
        data.push(b'\n');
        if let Some(emulator) = self.emulator_mut(task) {
            emulator.echo(&sent);
            emulator.scroll(Scroll::Bottom);
        }
        self.request(Request::Input { task, data });
        self.mark_dirty();
    }

    fn no_input_notice(&mut self, task: NodeId) {
        if self.tree.node(task).spec.accepts_input {
            self.notice("the task isn't running");
        } else {
            self.notice("this task doesn't read input");
        }
    }

    fn search_key(&mut self, task: NodeId, key: KeyEvent) {
        enum After {
            Search(Str),
            Step(Direction),
            Clear,
            Keep,
        }
        let View::Task(view) = &mut self.view else { return };
        let Mode::Search { query } = &mut view.mode else { return };
        let after = match key.code {
            KeyCode::Enter => {
                view.mode = Mode::Browse;
                After::Keep
            }
            KeyCode::Esc => {
                view.mode = Mode::Browse;
                After::Clear
            }
            KeyCode::Char('c' | 'f') if key.modifiers.contains(KeyModifiers::CTRL) => {
                view.mode = Mode::Browse;
                After::Keep
            }
            KeyCode::Up => After::Step(Direction::Left),
            KeyCode::Down => After::Step(Direction::Right),
            KeyCode::Backspace => {
                query.pop();
                After::Search(query.clone())
            }
            KeyCode::Char(c)
                if !key.modifiers.intersects(KeyModifiers::CTRL | KeyModifiers::ALT) =>
            {
                query.push(c);
                After::Search(query.clone())
            }
            _ => return,
        };
        if let Some(emulator) = self.emulator_mut(task) {
            match after {
                After::Search(query) => {
                    emulator.search(&query);
                }
                After::Step(direction) => {
                    emulator.search_step(direction);
                }
                After::Clear => emulator.clear_search(),
                After::Keep => {}
            }
        }
        self.mark_dirty();
    }

    pub(crate) fn task_mouse(&mut self, mouse: MouseEvent) {
        let Some(task) = self.task_view().map(|view| view.task) else { return };
        let content_rows = self.options.rows.saturating_sub(1);
        match mouse.kind {
            MouseEventKind::Scroll(direction) => {
                let delta = match direction {
                    ScrollDirection::Up => WHEEL_LINES,
                    ScrollDirection::Down => -WHEEL_LINES,
                    ScrollDirection::Left | ScrollDirection::Right => return,
                };
                if let Some(emulator) = self.emulator_mut(task) {
                    emulator.scroll(Scroll::Delta(delta));
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if mouse.row >= content_rows {
                    return;
                }
                if let Some(emulator) = self.emulator_mut(task) {
                    emulator.clear_selection();
                    emulator.select_begin(mouse.column, mouse.row);
                }
                if let Some(view) = self.task_view_mut() {
                    view.dragged = false;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let Some(emulator) = self.emulator_mut(task) else { return };
                if !emulator.has_selection() {
                    return;
                }
                // Dragging onto the edge rows scrolls, so a selection can be
                // longer than the screen.
                if mouse.row == 0 {
                    emulator.scroll(Scroll::Delta(1));
                } else if mouse.row >= content_rows {
                    emulator.scroll(Scroll::Delta(-1));
                }
                emulator.select_extend(mouse.column, mouse.row.min(content_rows.saturating_sub(1)));
                if let Some(view) = self.task_view_mut() {
                    view.dragged = true;
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let dragged = self.task_view().is_some_and(|view| view.dragged);
                let text = if dragged {
                    self.emulator_mut(task).and_then(|emulator| emulator.selected_text())
                } else {
                    None
                };
                if let Some(text) = text {
                    self.copy_to_clipboard(&text);
                    let count = text.chars().count();
                    self.notice(&vt_str::format!("copied {count} characters"));
                } else if let Some(emulator) = self.emulator_mut(task) {
                    emulator.clear_selection();
                }
            }
            _ => return,
        }
        self.mark_dirty();
    }
}
