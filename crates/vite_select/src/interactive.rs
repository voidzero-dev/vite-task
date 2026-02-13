use std::io::{Write, stdout};

use crossterm::{
    cursor::{self, MoveToColumn},
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    style::{Attribute, SetAttribute},
    terminal::{self, Clear, ClearType},
};
use owo_colors::{OwoColorize, Stream};

use crate::{RenderState, SelectItem, fuzzy::fuzzy_match};

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

/// Parameters for rendering a task list.
pub struct RenderParams<'a> {
    pub items: &'a [SelectItem],
    pub filtered: &'a [usize],
    /// Index into `filtered` of the highlighted item, or `None` for non-interactive.
    pub selected_in_filtered: Option<usize>,
    /// Which slice of `filtered` to display.
    pub visible_range: std::ops::Range<usize>,
    /// Number of items beyond the visible range.
    pub hidden_count: usize,
    pub header: Option<&'a str>,
    /// Current search text. `Some` enables the prompt line (interactive only).
    pub query: Option<&'a str>,
    /// `"\r\n"` for raw mode, `"\n"` for normal.
    pub line_ending: &'a str,
}

/// Render the item list. Shared rendering logic used by both interactive
/// and non-interactive modes (via [`crate::non_interactive`]).
///
/// Returns the number of lines written.
pub fn render_items(writer: &mut impl Write, params: &RenderParams<'_>) -> anyhow::Result<usize> {
    let RenderParams {
        items,
        filtered,
        selected_in_filtered,
        visible_range,
        hidden_count,
        header,
        query,
        line_ending,
    } = params;

    let mut lines = 0usize;

    // Header (e.g. error message)
    if let Some(header) = header {
        write!(writer, "{header}{line_ending}")?;
        lines += 1;
    }

    // Prompt line (interactive only)
    if let Some(q) = query {
        let bold = SetAttribute(Attribute::Bold);
        let reset = SetAttribute(Attribute::Reset);
        // Print ": " separator before query only when query is non-empty,
        // to avoid a trailing space that Windows ConPTY would strip.
        if q.is_empty() {
            write!(
                writer,
                "{bold}Search task{reset} (\u{2191}/\u{2193} to move, enter to select):{line_ending}",
            )?;
        } else {
            write!(
                writer,
                "{bold}Search task{reset} (\u{2191}/\u{2193} to move, enter to select): {q}{line_ending}",
            )?;
        }
        lines += 1;
    }

    // Items
    for vi in visible_range.clone() {
        let item_idx = filtered[vi];
        let item = &items[item_idx];
        let is_selected = *selected_in_filtered == Some(vi);
        let desc_str = item.description.as_str();
        let desc = desc_str.if_supports_color(Stream::Stdout, |s| s.cyan());

        if is_selected {
            write!(
                writer,
                "{bold}> {label}: {desc}{reset}{line_ending}",
                bold = SetAttribute(Attribute::Bold),
                label = item.label,
                reset = SetAttribute(Attribute::Reset),
            )?;
        } else {
            write!(writer, "  {}: {desc}{line_ending}", item.label)?;
        }
        lines += 1;
    }

    // Footer: hidden items count
    if *hidden_count > 0 {
        write!(writer, "  (\u{2026}{hidden_count} more){line_ending}")?;
        lines += 1;
    }

    // Empty state
    if filtered.is_empty() {
        write!(writer, "  No matching tasks.{line_ending}")?;
        lines += 1;
    }

    writer.flush()?;
    Ok(lines)
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

    let lines = render_items(
        stdout,
        &RenderParams {
            items: state.items,
            filtered: &state.filtered,
            selected_in_filtered: Some(state.selected),
            visible_range: state.visible_range(),
            hidden_count: state.hidden_count(),
            header,
            query: Some(&state.query),
            line_ending: "\r\n",
        },
    )?;

    state.rendered_lines = lines;
    Ok(())
}

pub fn run(
    items: &[SelectItem],
    initial_query: Option<&str>,
    selected_index: &mut usize,
    header: Option<&str>,
    page_size: usize,
    mut after_render: impl FnMut(&RenderState<'_>),
) -> anyhow::Result<()> {
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
                    // Clear the search query and reset the filter
                    state.query.clear();
                    state.refilter();
                }
                KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                    cleanup(&mut out, &state)?;
                    std::process::exit(130);
                }
                KeyCode::Enter => {
                    if let Some(idx) = state.selected_original_index() {
                        *selected_index = idx;
                    }
                    cleanup(&mut out, &state)?;
                    return Ok(());
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
