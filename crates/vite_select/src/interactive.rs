use std::io::{Write, stdout};

use crossterm::{
    cursor::{self, MoveToColumn},
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    style::{Attribute, Color, ResetColor, SetAttribute, SetForegroundColor},
    terminal::{self, Clear, ClearType},
};

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

/// Split a label like `"package#task"` into `("package#", "task")`.
/// Labels without `#` return `("", label)`.
fn split_label(label: &str) -> (&str, &str) {
    label.find('#').map_or(("", label), |pos| (&label[..=pos], &label[pos + 1..]))
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
    /// Maximum visible width per line. Descriptions are truncated to prevent
    /// line wrapping, which would break cursor-based clearing in interactive mode.
    /// Use `usize::MAX` to disable truncation (non-interactive / piped output).
    pub max_line_width: usize,
}

/// Render the item list. Shared rendering logic used by both interactive
/// and non-interactive modes (via [`crate::non_interactive`]).
///
/// Returns the number of lines written.
#[expect(clippy::too_many_lines, reason = "single rendering function with sequential layout logic")]
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
        max_line_width: _,
    } = params;

    let mut lines = 0usize;

    // Header (e.g. error message)
    if let Some(header) = header {
        write!(writer, "{header}{line_ending}")?;
        lines += 1;
    }

    let is_interactive = query.is_some();

    // Prompt line (interactive only)
    if let Some(q) = query {
        // Print ": " separator before query only when query is non-empty,
        // to avoid a trailing space that Windows ConPTY would strip.
        if q.is_empty() {
            write!(
                writer,
                "Select a task (\u{2191}/\u{2193}, Enter to run, Esc to clear):{line_ending}",
            )?;
        } else {
            write!(
                writer,
                "Select a task (\u{2191}/\u{2193}, Enter to run, Esc to clear): {q}{line_ending}",
            )?;
        }
        write!(writer, "{line_ending}")?;
        lines += 2;
    }

    // Compute max label width for interactive column alignment
    let max_label_width = if is_interactive {
        visible_range.clone().map(|vi| items[filtered[vi]].label.chars().count()).max().unwrap_or(0)
    } else {
        0
    };

    // Items
    for vi in visible_range.clone() {
        let item_idx = filtered[vi];
        let item = &items[item_idx];
        let is_selected = *selected_in_filtered == Some(vi);

        // Truncate description to prevent line wrapping.
        // Line layout:
        // - interactive prefix is "  › " or "    " (4 chars)
        // - non-interactive prefix is "  " (2 chars)
        // then label (padded to max_label_width in interactive) + ": " + description
        let label_width = item.label.chars().count();
        let padded_label_width = if is_interactive { max_label_width } else { label_width };
        let label_padding = padded_label_width - label_width;
        let prefix_width = if is_interactive { 4 } else { 2 };
        let prefix_and_label_width = prefix_width + padded_label_width + 2;
        let max_desc_chars = params.max_line_width.saturating_sub(prefix_and_label_width);
        let desc_str = item.description.as_str();
        let desc_char_count = desc_str.chars().count();
        let truncated;
        let display_desc = if desc_char_count > max_desc_chars {
            let take = max_desc_chars.saturating_sub(1); // room for "…"
            #[expect(clippy::disallowed_types, reason = "intermediate collect for char truncation")]
            let prefix: std::string::String = desc_str.chars().take(take).collect();
            truncated = vite_str::format!("{prefix}\u{2026}");
            truncated.as_str()
        } else {
            desc_str
        };

        if is_interactive {
            let (pkg, task) = split_label(&item.label);
            if is_selected {
                write!(
                    writer,
                    "{bold}  \u{203a} {pkg}{task}:{:>pad$} {desc}{no_attr}{line_ending}",
                    "",
                    pad = label_padding,
                    bold = SetAttribute(Attribute::Bold),
                    no_attr = SetAttribute(Attribute::Reset),
                    desc = display_desc,
                )?;
            } else {
                write!(
                    writer,
                    "    {light_cyan}{pkg}{no_color}{cyan}{task}{no_attr}:{:>pad$} {desc}{line_ending}",
                    "",
                    pad = label_padding,
                    light_cyan = SetForegroundColor(Color::Rgb { r: 130, g: 200, b: 210 }),
                    cyan = SetForegroundColor(Color::Cyan),
                    no_color = ResetColor,
                    no_attr = SetAttribute(Attribute::Reset),
                    desc = display_desc,
                )?;
            }
        } else if is_selected {
            write!(
                writer,
                "{bold}> {label}: {desc}{reset}{line_ending}",
                bold = SetAttribute(Attribute::Bold),
                label = item.label,
                desc = display_desc,
                reset = SetAttribute(Attribute::Reset),
            )?;
        } else {
            write!(writer, "  {}: {display_desc}{line_ending}", item.label)?;
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

    // Query terminal width on each render to handle resize
    let max_line_width = terminal::size().map_or(80, |(w, _)| w as usize);

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
            max_line_width,
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
    mut before_render: impl FnMut(&mut Vec<usize>, &str),
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
    before_render(&mut state.filtered, &state.query);

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
                    before_render(&mut state.filtered, &state.query);
                }
                KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                    cleanup(&mut out, &state)?;
                    std::process::exit(130);
                }
                KeyCode::Enter => {
                    let Some(idx) = state.selected_original_index() else {
                        continue;
                    };
                    *selected_index = idx;
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
                    before_render(&mut state.filtered, &state.query);
                }
                KeyCode::Backspace => {
                    state.query.pop();
                    state.refilter();
                    before_render(&mut state.filtered, &state.query);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_items(items: &[(&str, &str)]) -> Vec<SelectItem> {
        items
            .iter()
            .map(|(label, desc)| SelectItem { label: (*label).into(), description: (*desc).into() })
            .collect()
    }

    /// Strip ANSI escape sequences from output for easier assertions.
    #[expect(clippy::disallowed_types, reason = "test helper building arbitrary output string")]
    fn strip_ansi(s: &str) -> String {
        let mut result = String::new();
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                // Skip until we hit a letter (end of escape sequence)
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                result.push(c);
            }
        }
        result
    }

    #[expect(clippy::disallowed_types, reason = "test helper building arbitrary output string")]
    fn render_to_string(items: &[SelectItem], max_line_width: usize) -> String {
        let filtered: Vec<usize> = (0..items.len()).collect();
        let len = filtered.len();
        let mut buf = Vec::new();
        render_items(
            &mut buf,
            &RenderParams {
                items,
                filtered: &filtered,
                selected_in_filtered: Some(0),
                visible_range: 0..len,
                hidden_count: 0,
                header: None,
                query: None,
                line_ending: "\n",
                max_line_width,
            },
        )
        .unwrap();
        strip_ansi(&String::from_utf8(buf).unwrap())
    }

    #[expect(clippy::disallowed_types, reason = "test helper building arbitrary output string")]
    fn render_interactive_to_string(
        items: &[SelectItem],
        query: &str,
        max_line_width: usize,
    ) -> String {
        let filtered: Vec<usize> = (0..items.len()).collect();
        let len = filtered.len();
        let mut buf = Vec::new();
        render_items(
            &mut buf,
            &RenderParams {
                items,
                filtered: &filtered,
                selected_in_filtered: Some(0),
                visible_range: 0..len,
                hidden_count: 0,
                header: None,
                query: Some(query),
                line_ending: "\n",
                max_line_width,
            },
        )
        .unwrap();
        strip_ansi(&String::from_utf8(buf).unwrap())
    }

    #[test]
    fn truncates_long_description() {
        let items = make_items(&[("build", "a]really long command that exceeds the width limit")]);
        //                        "  build: a really long..." = 2 + 5 + 2 + desc
        // max_line_width = 30 => max_desc = 30 - 9 = 21 chars
        let output = render_to_string(&items, 30);
        let line = output.lines().next().unwrap();
        // "> " (2) + "build" (5) + ": " (2) + desc (21) = 30
        assert!(
            line.chars().count() <= 30,
            "line should be at most 30 chars, got {}: {line:?}",
            line.chars().count()
        );
        assert!(line.contains('\u{2026}'), "truncated line should contain ellipsis: {line:?}");
    }

    #[test]
    fn does_not_truncate_short_description() {
        let items = make_items(&[("build", "echo ok")]);
        let output = render_to_string(&items, 80);
        let line = output.lines().next().unwrap();
        assert!(!line.contains('\u{2026}'), "short line should not be truncated: {line:?}");
        assert!(line.contains("echo ok"), "full description should appear: {line:?}");
    }

    #[test]
    fn max_line_width_max_disables_truncation() {
        let long_desc = "x".repeat(500);
        let items = make_items(&[("build", &long_desc)]);
        let output = render_to_string(&items, usize::MAX);
        let line = output.lines().next().unwrap();
        assert!(!line.contains('\u{2026}'), "usize::MAX should disable truncation: {line:?}");
        assert!(line.contains(&long_desc), "full 500-char description should appear");
    }

    #[test]
    fn each_line_fits_within_max_width() {
        let items = make_items(&[
            ("build", "tsc -p tsconfig.build.json && echo done"),
            ("lint", "oxlint --fix"),
            ("test", "vitest run --reporter=verbose --coverage"),
        ]);
        let max_width = 40;
        let output = render_to_string(&items, max_width);
        for line in output.lines() {
            assert!(
                line.chars().count() <= max_width,
                "line exceeds max width {max_width}: ({}) {line:?}",
                line.chars().count()
            );
        }
    }

    #[test]
    fn truncation_preserves_label() {
        let items = make_items(&[("my-task", "very long description here")]);
        // "  my-task: very..." => prefix(2) + label(7) + sep(2) + desc
        // max_line_width = 20 => max_desc = 20 - 11 = 9 chars
        let output = render_to_string(&items, 20);
        let line = output.lines().next().unwrap();
        assert!(line.contains("my-task"), "label should always be preserved: {line:?}");
    }

    #[test]
    fn interactive_style_matches_vp_selector_marker_and_indent() {
        let items = make_items(&[("build", "echo build"), ("lint", "echo lint")]);
        let output = render_interactive_to_string(&items, "", 80);
        let mut lines = output.lines();
        let prompt = lines.next().unwrap();
        let spacer = lines.next().unwrap();
        let selected = lines.next().unwrap();
        let unselected = lines.next().unwrap();
        assert_eq!(prompt, "Select a task (↑/↓, Enter to run, Esc to clear):");
        assert!(spacer.is_empty());
        assert_eq!(selected, "  › build: echo build");
        assert_eq!(unselected, "    lint:  echo lint");
    }

    #[test]
    fn interactive_commands_are_aligned() {
        let items =
            make_items(&[("build", "echo build"), ("lint", "echo lint"), ("test", "vitest run")]);
        let output = render_interactive_to_string(&items, "", 80);
        let item_lines: Vec<&str> = output.lines().skip(2).collect();
        // max_label_width = 5 ("build")
        // prefix(4) + max_label(5) + ":" + padding + " " + desc
        assert_eq!(item_lines[0], "  \u{203a} build: echo build");
        assert_eq!(item_lines[1], "    lint:  echo lint");
        assert_eq!(item_lines[2], "    test:  vitest run");
    }

    #[test]
    fn interactive_alignment_with_package_labels() {
        let items = make_items(&[("app#build", "echo build"), ("lint", "echo lint")]);
        let output = render_interactive_to_string(&items, "", 80);
        let item_lines: Vec<&str> = output.lines().skip(2).collect();
        // max_label_width = 9 ("app#build"), padding for "lint" = 5
        assert_eq!(item_lines[0], "  \u{203a} app#build: echo build");
        assert_eq!(item_lines[1], "    lint:      echo lint");
    }

    #[test]
    fn interactive_truncation_accounts_for_padding() {
        let items = make_items(&[
            ("build", "a really long command that exceeds the width limit"),
            ("lint", "short"),
        ]);
        // max_label_width = 5, prefix(4) + max_label(5) + sep(2) = 11
        // max_line_width = 30 => max_desc = 30 - 11 = 19 chars
        let output = render_interactive_to_string(&items, "", 30);
        for line in output.lines().skip(2) {
            assert!(
                line.chars().count() <= 30,
                "line exceeds 30 chars: ({}) {line:?}",
                line.chars().count()
            );
        }
        let build_line = output.lines().nth(2).unwrap();
        assert!(
            build_line.contains('\u{2026}'),
            "long description should be truncated: {build_line:?}"
        );
    }

    #[test]
    fn interactive_padding_aligns_commands() {
        let items = make_items(&[
            ("app#build", "echo build"),
            ("app#lint", "echo lint"),
            ("lib#typecheck", "echo tc"),
        ]);
        let output = render_interactive_to_string(&items, "", 80);
        let item_lines: Vec<&str> = output.lines().skip(2).collect();
        // max_label_width = 13 ("lib#typecheck")
        // Padding goes after ":" to align commands
        assert_eq!(item_lines[0], "  \u{203a} app#build:     echo build");
        assert_eq!(item_lines[1], "    app#lint:      echo lint");
        assert_eq!(item_lines[2], "    lib#typecheck: echo tc");

        // Verify all commands start at the same char column
        // prefix(4) + max_label(13) + ":" + padding + " " = commands start at column 19
        let cmd_columns: Vec<usize> = item_lines
            .iter()
            .map(|l| {
                let colon_pos = l.chars().take_while(|&c| c != ':').count();
                // skip colon, then count padding spaces + the separator space
                colon_pos
                    + 1
                    + l[l.find(':').unwrap() + 1..].chars().take_while(|&c| c == ' ').count()
            })
            .collect();
        assert!(
            cmd_columns.windows(2).all(|w| w[0] == w[1]),
            "command columns should be aligned: {cmd_columns:?}"
        );
    }

    #[test]
    fn interactive_padding_with_truncation_preserves_ellipsis() {
        let items = make_items(&[
            ("app#build", "a really long command that exceeds the width limit"),
            ("lint", "short"),
        ]);
        // max_label_width = 9 ("app#build"), prefix(4) + 9 + sep(2) = 15
        // max_line_width = 30 => max_desc = 30 - 15 = 15 chars
        let output = render_interactive_to_string(&items, "", 30);
        for line in output.lines().skip(2) {
            assert!(
                line.chars().count() <= 30,
                "line exceeds 30 chars: ({}) {line:?}",
                line.chars().count()
            );
        }
        let build_line = output.lines().nth(2).unwrap();
        assert!(
            build_line.contains('\u{2026}'),
            "truncated line should contain ellipsis: {build_line:?}"
        );
        // "lint" (4) has padding of 5 after colon (9 - 4)
        let lint_line = output.lines().nth(3).unwrap();
        assert!(
            lint_line.contains("lint:      short"),
            "short label should have padding after colon: {lint_line:?}"
        );
    }
}
