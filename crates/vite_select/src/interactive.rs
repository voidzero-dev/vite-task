use std::io::{Write, stdout};

use crossterm::{
    cursor::{self, MoveToColumn},
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor},
    terminal::{self, Clear, ClearType},
};

use crate::{RenderState, SelectItem, SelectResult, fuzzy::fuzzy_match};

struct RawModeGuard;

impl RawModeGuard {
    fn enable() -> anyhow::Result<Self> {
        terminal::enable_raw_mode()?;
        Ok(Self)
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
    }
}

struct State<'a> {
    items: &'a [SelectItem],
    /// Indices into `items` that match the current query, in score order.
    filtered: Vec<usize>,
    #[expect(
        clippy::disallowed_types,
        reason = "crossterm key events push chars one at a time; String is natural here"
    )]
    query: String,
    /// Index into `filtered`.
    selected: usize,
    /// First visible row in the filtered list (scroll offset).
    scroll_offset: usize,
    page_size: usize,
    /// Number of lines rendered in the last frame (for clearing).
    rendered_lines: usize,
}

impl<'a> State<'a> {
    fn new(items: &'a [SelectItem], initial_query: Option<&str>, page_size: usize) -> Self {
        let query = initial_query.unwrap_or_default().to_owned();
        let mut state = Self {
            items,
            filtered: Vec::new(),
            query,
            selected: 0,
            scroll_offset: 0,
            page_size,
            rendered_lines: 0,
        };
        state.refilter();
        state
    }

    fn refilter(&mut self) {
        let labels: Vec<&str> = self.items.iter().map(|i| i.label.as_str()).collect();
        self.filtered = fuzzy_match(&self.query, &labels);
        self.selected = 0;
        self.scroll_offset = 0;
    }

    const fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
            if self.selected < self.scroll_offset {
                self.scroll_offset = self.selected;
            }
        }
    }

    const fn move_down(&mut self) {
        if !self.filtered.is_empty() && self.selected < self.filtered.len() - 1 {
            self.selected += 1;
            if self.selected >= self.scroll_offset + self.page_size {
                self.scroll_offset = self.selected + 1 - self.page_size;
            }
        }
    }

    fn selected_original_index(&self) -> Option<usize> {
        self.filtered.get(self.selected).copied()
    }

    fn visible_range(&self) -> std::ops::Range<usize> {
        let end = (self.scroll_offset + self.page_size).min(self.filtered.len());
        self.scroll_offset..end
    }

    const fn hidden_count(&self) -> usize {
        self.filtered.len().saturating_sub(self.scroll_offset + self.page_size)
    }
}

fn render(
    stdout: &mut impl Write,
    state: &mut State<'_>,
    header: Option<&str>,
) -> anyhow::Result<()> {
    // Move cursor up to clear previous render
    if state.rendered_lines > 0 {
        let move_up = u16::try_from(state.rendered_lines)
            .expect("rendered_lines fits in u16: at most header + page_size + footer lines");
        crossterm::execute!(
            stdout,
            cursor::MoveUp(move_up),
            MoveToColumn(0),
            Clear(ClearType::FromCursorDown),
        )?;
    }

    let mut lines = 0u16;

    // Header (error message)
    if let Some(header) = header {
        crossterm::execute!(stdout, Print(header), Print("\r\n"))?;
        lines += 1;
    }

    // Prompt line
    crossterm::execute!(
        stdout,
        SetAttribute(Attribute::Bold),
        Print("Search task"),
        SetAttribute(Attribute::Reset),
        Print(" ("),
        Print("\u{2191}/\u{2193} to move, enter to select"),
        Print("): "),
        Print(&state.query),
        Print("\r\n"),
    )?;
    lines += 1;

    // Items
    let visible = state.visible_range();

    for vi in visible {
        let item_idx = state.filtered[vi];
        let item = &state.items[item_idx];
        let is_selected = vi == state.selected;

        if is_selected {
            crossterm::execute!(
                stdout,
                SetAttribute(Attribute::Bold),
                Print("> "),
                Print(item.label.as_str()),
                Print(": "),
                SetForegroundColor(Color::Cyan),
                Print(item.description.as_str()),
                ResetColor,
                SetAttribute(Attribute::Reset),
                Print("\r\n"),
            )?;
        } else {
            crossterm::execute!(
                stdout,
                Print("  "),
                Print(item.label.as_str()),
                Print(": "),
                SetForegroundColor(Color::Cyan),
                Print(item.description.as_str()),
                ResetColor,
                Print("\r\n"),
            )?;
        }
        lines += 1;
    }

    // Footer: hidden items count
    let hidden = state.hidden_count();
    if hidden > 0 {
        crossterm::execute!(
            stdout,
            Print(vite_str::format!("  (\u{2026}{hidden} more)")),
            Print("\r\n"),
        )?;
        lines += 1;
    }

    // Empty state
    if state.filtered.is_empty() {
        crossterm::execute!(stdout, Print("  No matching tasks.\r\n"))?;
        lines += 1;
    }

    stdout.flush()?;
    state.rendered_lines = lines as usize;
    Ok(())
}

pub fn run(
    items: &[SelectItem],
    initial_query: Option<&str>,
    header: Option<&str>,
    page_size: usize,
    mut after_render: impl FnMut(&RenderState<'_>),
) -> anyhow::Result<Option<SelectResult>> {
    if items.is_empty() {
        anyhow::bail!("No tasks available");
    }

    let _guard = RawModeGuard::enable()?;
    // Hide cursor while the widget is active
    let mut out = stdout();
    crossterm::execute!(out, cursor::Hide)?;

    let mut state = State::new(items, initial_query, page_size);

    // Initial render
    render(&mut out, &mut state, header)?;
    after_render(&RenderState { query: &state.query, selected_index: state.selected });

    loop {
        let ev = event::read()?;
        match ev {
            Event::Key(KeyEvent { code, modifiers, kind: KeyEventKind::Press, .. }) => match code {
                KeyCode::Esc => {
                    cleanup(&mut out, &state)?;
                    return Ok(None);
                }
                KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                    cleanup(&mut out, &state)?;
                    return Ok(None);
                }
                KeyCode::Enter => {
                    let result = state
                        .selected_original_index()
                        .map(|idx| SelectResult { original_index: idx });
                    cleanup(&mut out, &state)?;
                    return Ok(result);
                }
                KeyCode::Up => {
                    state.move_up();
                }
                KeyCode::Down => {
                    state.move_down();
                }
                KeyCode::Char(c) => {
                    state.query.push(c);
                    state.refilter();
                }
                KeyCode::Backspace => {
                    state.query.pop();
                    state.refilter();
                }
                _ => continue,
            },
            _ => continue,
        }

        render(&mut out, &mut state, header)?;
        after_render(&RenderState { query: &state.query, selected_index: state.selected });
    }
}

/// Clear the widget output and restore cursor.
fn cleanup(stdout: &mut impl Write, state: &State<'_>) -> anyhow::Result<()> {
    if state.rendered_lines > 0 {
        let move_up = u16::try_from(state.rendered_lines)
            .expect("rendered_lines fits in u16: at most header + page_size + footer lines");
        crossterm::execute!(
            stdout,
            cursor::MoveUp(move_up),
            MoveToColumn(0),
            Clear(ClearType::FromCursorDown),
        )?;
    }
    crossterm::execute!(stdout, cursor::Show)?;
    stdout.flush()?;
    Ok(())
}
