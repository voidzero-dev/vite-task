//! The task tree, drawn in the primary screen.

use std::time::Duration;

use ratatui_core::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
};
use terminput::{KeyCode, KeyEvent};
use unicode_width::UnicodeWidthStr as _;
use vt_str::Str;

use crate::{
    ansi::encode_rows,
    tree::{Effective, NodeId, TaskState, Tone},
    tui::{RowKey, SPINNER, Tui},
};

pub const DIM: Style = Style::new().add_modifier(Modifier::DIM);
const KEY: Style = Style::new();
const SEPARATOR: &str = " · ";

/// Text pieces with their styles.
type Spans = Vec<(Str, Style)>;

impl Tui {
    /// Every row of the tree, before it's cut to fit the screen. Siblings that
    /// finished cleanly are folded into one row when the tree doesn't fit.
    pub(crate) fn all_rows(&self) -> Vec<RowKey> {
        let rows = self.visible_rows(false);
        if rows.len() <= self.tree_capacity() { rows } else { self.visible_rows(true) }
    }

    fn tree_capacity(&self) -> usize {
        usize::from(self.options.rows).saturating_sub(1).max(1)
    }

    fn visible_rows(&self, fold_done: bool) -> Vec<RowKey> {
        let mut rows = Vec::new();
        self.push_rows(None, &self.tree.roots, fold_done, &mut rows);
        rows
    }

    fn push_rows(
        &self,
        parent: Option<NodeId>,
        children: &[NodeId],
        fold_done: bool,
        rows: &mut Vec<RowKey>,
    ) {
        let show_done =
            parent.map_or(self.show_done_roots, |parent| self.tree.node(parent).show_done_children);
        let mut index = 0;
        while index < children.len() {
            let id = children[index];
            if fold_done && !show_done {
                let mut end = index;
                while end < children.len()
                    && self.tree.is_clean_success(children[end])
                    && !self.contains_selection(children[end])
                {
                    end += 1;
                }
                if end - index >= 2 {
                    rows.push(RowKey::Done { parent, first: id, count: end - index });
                    index = end;
                    continue;
                }
            }
            rows.push(RowKey::Node(id));
            let node = self.tree.node(id);
            if node.is_group() && self.is_row_expanded(id) {
                self.push_rows(Some(id), &node.children, fold_done, rows);
            }
            index += 1;
        }
    }

    /// Groups the tree folds on its own stay open while the selection is in them.
    fn is_row_expanded(&self, id: NodeId) -> bool {
        self.tree.is_expanded(id)
            || (self.tree.node(id).expanded.is_none() && self.contains_selection(id))
    }

    /// Whether the selected node is `id` or below it.
    fn contains_selection(&self, id: NodeId) -> bool {
        let Some(RowKey::Node(mut selected)) = self.selected else { return false };
        loop {
            if selected == id {
                return true;
            }
            match self.tree.node(selected).parent {
                Some(parent) => selected = parent,
                None => return false,
            }
        }
    }

    /// The row to keep on screen: the selection, or else where the run is.
    fn focus_index(&self, rows: &[RowKey]) -> usize {
        if let Some(selected) = self.selected
            && let Some(index) = rows.iter().position(|row| row.same_row(selected))
        {
            return index;
        }
        let find = |wanted: &dyn Fn(TaskState) -> bool| {
            rows.iter().position(|row| match row {
                RowKey::Node(id) => {
                    !self.tree.node(*id).is_group() && wanted(self.tree.effective(*id).state)
                }
                _ => false,
            })
        };
        find(&|state| state == TaskState::Failed)
            .or_else(|| find(&|state| matches!(state, TaskState::Running | TaskState::Ready)))
            .or_else(|| find(&|state| !state.is_finished()))
            .unwrap_or(0)
    }

    /// The rows that fit on screen, with a row saying how many are cut off
    /// above and below.
    fn layout(&mut self) -> Vec<RowKey> {
        let rows = self.all_rows();
        let capacity = self.tree_capacity();
        let total = rows.len();
        if total <= capacity {
            self.scroll_top = 0;
            return rows;
        }
        let focus = self.focus_index(&rows);
        let inner = capacity.saturating_sub(2).max(1);
        let mut top = self.scroll_top;
        if focus < top {
            top = focus;
        } else if focus >= top + inner {
            top = focus + 1 - inner;
        }
        top = top.min(total.saturating_sub(inner));
        self.scroll_top = top;
        let (start, end) = if top == 0 {
            (0, capacity - 1)
        } else if top + inner >= total {
            (total - (capacity - 1), total)
        } else {
            (top, top + inner)
        };
        let mut window = Vec::with_capacity(capacity);
        if start > 0 {
            window.push(RowKey::More { above: true, count: start });
        }
        window.extend_from_slice(&rows[start..end]);
        if end < total {
            window.push(RowKey::More { above: false, count: total - end });
        }
        window
    }

    /// Encoded rows of the tree. Interactive frames have a column for the
    /// selection marker and a row of key hints; the final frame has neither.
    /// Without decorations, neither is drawn until a key selects a row, and
    /// statuses follow their labels instead of lining up.
    pub(crate) fn tree_rows(&mut self, interactive: bool) -> Vec<Vec<u8>> {
        let rows = self.layout();
        let width = self.options.cols;
        let decorations = self.options.decorations;
        let hints = interactive && decorations;
        let height = rows.len() + usize::from(hints);
        let mut buffer =
            Buffer::empty(Rect::new(0, 0, width, u16::try_from(height).unwrap_or(u16::MAX)));
        let gutter: u16 =
            if interactive && (decorations || self.selected.is_some()) { 2 } else { 0 };

        let lines: Vec<(u16, Spans, Spans)> =
            rows.iter().map(|&row| self.row_content(row)).collect();
        let label_end = lines
            .iter()
            .filter(|(_, _, status)| !status.is_empty())
            .map(|(indent, label, _)| gutter + indent + spans_width(label))
            .max()
            .unwrap_or(0);
        let status_col = if decorations {
            (label_end + 2).min(width.saturating_mul(3) / 5).max(gutter + 4)
        } else {
            0
        };

        for (y, (&row, (indent, label, status))) in rows.iter().zip(&lines).enumerate() {
            let y = u16::try_from(y).unwrap_or(u16::MAX);
            let selected =
                interactive && self.selected.is_some_and(|selected| selected.same_row(row));
            if selected {
                buffer.set_string(
                    0,
                    y,
                    "›",
                    Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                );
            }
            // A label longer than the status column pushes its own status
            // right. It's only cut short when the row can't fit both.
            let label_width = gutter + indent + spans_width(label);
            let status_width = spans_width(status)
                + u16::try_from(status.len().saturating_sub(1) * SEPARATOR.width()).unwrap_or(0);
            let label_limit = if status.is_empty() || label_width + 2 + status_width <= width {
                width
            } else {
                width.saturating_sub(status_width + 2).max(status_col.saturating_sub(1))
            };
            let mut x = gutter + indent;
            for (index, (text, style)) in label.iter().enumerate() {
                // The first span is the icon; the selection makes the rest bold.
                let style =
                    if selected && index > 0 { style.add_modifier(Modifier::BOLD) } else { *style };
                x = put(&mut buffer, x, y, text, style, label_limit);
            }
            let mut x = (x + 2).max(status_col);
            for (index, (text, style)) in status.iter().enumerate() {
                if index > 0 {
                    x = put(&mut buffer, x, y, SEPARATOR, DIM, width);
                }
                x = put(&mut buffer, x, y, text, *style, width);
            }
        }
        if hints {
            self.draw_hints(&mut buffer, u16::try_from(height - 1).unwrap_or(0));
        }
        encode_rows(&buffer, decorations)
    }

    fn draw_hints(&self, buffer: &mut Buffer, y: u16) {
        let width = self.options.cols;
        let hints: &[(&str, &str)] = if self.selected.is_some() {
            &[("↑↓", "move"), ("←→", "fold"), ("enter", "output"), ("ctrl-c", "stop")]
        } else {
            &[("↑↓", "select a task"), ("ctrl-c", "stop")]
        };
        let name_width = u16::try_from(self.options.run_name.width()).unwrap_or(0);
        let limit = width.saturating_sub(name_width + 2);
        let mut x = 2;
        for (index, (key, description)) in hints.iter().enumerate() {
            if index > 0 {
                x = put(buffer, x, y, "   ", DIM, limit);
            }
            x = put(buffer, x, y, key, KEY, limit);
            x = put(buffer, x, y, " ", DIM, limit);
            x = put(buffer, x, y, description, DIM, limit);
        }
        if x + name_width + 2 <= width {
            put(buffer, width - name_width, y, &self.options.run_name, DIM, width);
        }
    }

    /// Indent, label spans (icon first) and status spans of a row.
    fn row_content(&self, row: RowKey) -> (u16, Spans, Spans) {
        match row {
            RowKey::Node(id) => {
                let node = self.tree.node(id);
                let effective = self.tree.effective(id);
                let (icon, icon_style) = self.icon(effective);
                let mut label = vec![(Str::from(icon), icon_style), (Str::from(" "), Style::new())];
                label.push((node.spec.label.clone(), Style::new()));
                if let Some(command) = &node.spec.command {
                    label.push((Str::from(": "), DIM));
                    label.push((command.clone(), Style::new()));
                }
                if node.is_group() && !self.is_row_expanded(id) {
                    label.push((Str::from(" ▸"), DIM));
                }
                (node.depth * 2, label, self.status_spans(id))
            }
            RowKey::Done { first, count, .. } => {
                let depth = self.tree.node(first).depth;
                let label = vec![
                    (Str::from("✓"), Style::new().fg(Color::Green)),
                    (Str::from(" "), Style::new()),
                    (vt_str::format!("{count} more done"), DIM),
                ];
                (depth * 2, label, Vec::new())
            }
            RowKey::More { above, count } => {
                let arrow = if above { "↑" } else { "↓" };
                (0, vec![(vt_str::format!("{arrow} {count} more"), DIM)], Vec::new())
            }
        }
    }

    pub(crate) fn icon(&self, effective: Effective) -> (&'static str, Style) {
        let dim = DIM;
        let tone_color = match effective.tone {
            Tone::Normal => Color::Green,
            Tone::Warning => Color::Yellow,
            Tone::Error => Color::Red,
        };
        match effective.state {
            TaskState::Waiting | TaskState::Queued => ("○", dim),
            TaskState::Running => {
                let icon = if self.options.animate { SPINNER[self.spinner] } else { "●" };
                (icon, Style::new().fg(Color::Cyan))
            }
            TaskState::Ready => ("◉", Style::new().fg(tone_color)),
            TaskState::Succeeded | TaskState::CacheHit => {
                if effective.tone == Tone::Warning {
                    ("!", Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD))
                } else {
                    ("✓", Style::new().fg(Color::Green))
                }
            }
            TaskState::Failed => ("✗", Style::new().fg(Color::Red).add_modifier(Modifier::BOLD)),
            TaskState::Cancelled => ("⊘", dim),
            TaskState::NotRun => ("–", dim),
        }
    }

    /// The pieces shown at the end of a row, joined with ` · `.
    pub(crate) fn status_spans(&self, id: NodeId) -> Spans {
        let node = self.tree.node(id);
        let effective = self.tree.effective(id);
        let tone_style = match node.tone {
            Tone::Normal => DIM,
            Tone::Warning => Style::new().fg(Color::Yellow),
            Tone::Error => Style::new().fg(Color::Red),
        };
        let mut spans = Vec::new();
        if node.is_group() {
            if let Some(status) = &node.status {
                spans.push((status.clone(), tone_style));
            } else if self.options.live_status
                && effective.state.is_started()
                && !effective.state.is_finished()
                && effective.state != TaskState::Ready
            {
                let (done, total) = self.tree.progress(id);
                spans.push((vt_str::format!("{done}/{total}"), DIM));
            }
            if matches!(effective.state, TaskState::Succeeded | TaskState::Failed)
                && let (Some(start), Some(finish)) = self.tree.span(id)
            {
                spans.push((format_duration(finish.saturating_sub(start)), DIM));
            }
            return spans;
        }
        if node.state == TaskState::CacheHit {
            spans.push((Str::from("cache hit"), DIM));
        }
        let live = self.options.live_status || node.state != TaskState::Running;
        if let Some(status) = node.status.as_ref().filter(|_| live) {
            spans.push((status.clone(), tone_style));
        } else if node.state == TaskState::Cancelled {
            spans.push((Str::from("cancelled"), DIM));
        } else if node.state == TaskState::NotRun {
            spans.push((Str::from("not run"), DIM));
        }
        if matches!(node.state, TaskState::Succeeded | TaskState::Failed)
            && let (Some(start), Some(finish)) = (node.started_at, node.finished_at)
        {
            spans.push((format_duration(finish.saturating_sub(start)), DIM));
        }
        spans
    }

    /// The name a process goes by in the task view and the report.
    pub(crate) fn title(&self, id: NodeId) -> Str {
        let node = self.tree.node(id);
        if let Some(command) = &node.spec.command {
            return vt_str::format!("{}: {}", node.spec.label, command);
        }
        node.parent.map_or_else(
            || node.spec.label.clone(),
            |parent| vt_str::format!("{} › {}", self.tree.node(parent).spec.label, node.spec.label),
        )
    }

    pub(crate) fn tree_key(&mut self, key: KeyEvent) {
        let rows = self.all_rows();
        if rows.is_empty() {
            return;
        }
        let current =
            self.selected.and_then(|selected| rows.iter().position(|row| row.same_row(selected)));
        let Some(index) = current else {
            // The first key only shows the selection, where the run is.
            if matches!(
                key.code,
                KeyCode::Up
                    | KeyCode::Down
                    | KeyCode::Char('j' | 'k')
                    | KeyCode::Enter
                    | KeyCode::Left
                    | KeyCode::Right
            ) {
                self.selected = Some(rows[self.focus_index(&rows)]);
                self.mark_dirty();
            }
            return;
        };
        let row = rows[index];
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.select(rows[index.saturating_sub(1)]),
            KeyCode::Down | KeyCode::Char('j') => {
                self.select(rows[(index + 1).min(rows.len() - 1)]);
            }
            KeyCode::Home | KeyCode::Char('g') => self.select(rows[0]),
            KeyCode::End | KeyCode::Char('G') => self.select(rows[rows.len() - 1]),
            KeyCode::Left | KeyCode::Char('h') => match row {
                RowKey::Node(id) if self.tree.node(id).is_group() && self.is_row_expanded(id) => {
                    self.tree.node_mut(id).expanded = Some(false);
                    self.mark_dirty();
                }
                RowKey::Node(id) => {
                    if let Some(parent) = self.tree.node(id).parent {
                        self.select(RowKey::Node(parent));
                    }
                }
                RowKey::Done { parent: Some(parent), .. } => self.select(RowKey::Node(parent)),
                _ => {}
            },
            KeyCode::Right | KeyCode::Char('l') => match row {
                RowKey::Node(id) if self.tree.node(id).is_group() && !self.is_row_expanded(id) => {
                    self.tree.node_mut(id).expanded = Some(true);
                    self.mark_dirty();
                }
                RowKey::Done { .. } => self.unfold_done(row),
                _ => {}
            },
            KeyCode::Enter | KeyCode::Char(' ') => match row {
                RowKey::Node(id) if self.tree.node(id).is_group() => {
                    let expanded = self.is_row_expanded(id);
                    self.tree.node_mut(id).expanded = Some(!expanded);
                    self.mark_dirty();
                }
                RowKey::Node(id) => self.open_task(id),
                RowKey::Done { .. } => self.unfold_done(row),
                RowKey::More { .. } => {}
            },
            _ => {}
        }
    }

    const fn select(&mut self, row: RowKey) {
        self.selected = Some(row);
        self.mark_dirty();
    }

    fn unfold_done(&mut self, row: RowKey) {
        let RowKey::Done { parent, first, .. } = row else { return };
        match parent {
            Some(parent) => self.tree.node_mut(parent).show_done_children = true,
            None => self.show_done_roots = true,
        }
        self.select(RowKey::Node(first));
    }
}

/// Writes `text` at `x` without going past `limit`, ending with `…` when it
/// doesn't fit. Returns the column after the text.
pub fn put(buffer: &mut Buffer, x: u16, y: u16, text: &str, style: Style, limit: u16) -> u16 {
    let limit = limit.min(buffer.area.width);
    if x >= limit {
        return x;
    }
    let available = usize::from(limit - x);
    let width = text.width();
    if width <= available {
        let (end, _) = buffer.set_stringn(x, y, text, available, style);
        return end;
    }
    let (end, _) = buffer.set_stringn(x, y, text, available.saturating_sub(1), style);
    buffer.set_string(end, y, "…", style);
    end + 1
}

fn spans_width(spans: &[(Str, Style)]) -> u16 {
    spans.iter().map(|(text, _)| u16::try_from(text.width()).unwrap_or(u16::MAX)).sum()
}

pub fn format_duration(duration: Duration) -> Str {
    let millis = duration.as_millis();
    if millis < 1000 {
        vt_str::format!("{millis}ms")
    } else if millis < 60_000 {
        vt_str::format!("{}.{}s", millis / 1000, (millis % 1000) / 100)
    } else {
        let seconds = millis / 1000;
        vt_str::format!("{}m {:02}s", seconds / 60, seconds % 60)
    }
}
