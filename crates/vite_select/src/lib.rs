mod fuzzy;
mod interactive;

use std::io::Write;

pub use fuzzy::fuzzy_match;
use interactive::{RenderParams, render_items};
use vite_str::Str;

/// An item in the selection list.
pub struct SelectItem {
    /// Display label, e.g. `"build"` or `"app#build"`.
    pub label: Str,
    /// Description shown next to the label, e.g. `"echo build app"`.
    pub description: Str,
}

/// Selection mode.
pub enum Mode<'a> {
    /// Interactive terminal UI with fuzzy search, keyboard navigation, and selection.
    ///
    /// On Enter, `*selected_index` is set to the index of the chosen item
    /// in the original `items` slice.
    Interactive { selected_index: &'a mut usize },
    /// Non-interactive: renders the list once and returns.
    NonInteractive,
}

/// Snapshot of the selector's visible state, passed to `after_render`.
pub struct RenderState<'a> {
    /// Current search text (empty if no filter typed yet).
    pub query: &'a str,
    /// Index of the highlighted item in the **filtered** list.
    pub selected_index: usize,
}

/// Show a task selection list.
///
/// In [`Mode::Interactive`], enters a terminal UI with fuzzy search and
/// keyboard navigation. `after_render` is called after every render with the
/// current visible state (useful for emitting test milestones). On Enter,
/// `*selected_index` is set to the chosen item's index in the original
/// `items` slice.
///
/// In [`Mode::NonInteractive`], renders the list once to `writer` and
/// returns. `page_size` and `after_render` are ignored.
///
/// # Errors
///
/// Returns an error if terminal I/O fails.
pub fn select_list(
    writer: &mut impl Write,
    items: &[SelectItem],
    query: Option<&str>,
    mode: Mode<'_>,
    header: Option<&str>,
    page_size: usize,
    after_render: impl FnMut(&RenderState<'_>),
) -> anyhow::Result<()> {
    match mode {
        Mode::Interactive { selected_index } => {
            interactive::run(items, query, selected_index, header, page_size, after_render)
        }
        Mode::NonInteractive => non_interactive(writer, items, query, header),
    }
}

fn non_interactive(
    writer: &mut impl Write,
    items: &[SelectItem],
    query: Option<&str>,
    header: Option<&str>,
) -> anyhow::Result<()> {
    let labels: Vec<&str> = items.iter().map(|item| item.label.as_str()).collect();
    let filtered: Vec<usize> =
        query.map_or_else(|| (0..items.len()).collect(), |q| fuzzy_match(q, &labels));
    let len = filtered.len();

    render_items(
        writer,
        &RenderParams {
            items,
            filtered: &filtered,
            selected_in_filtered: None,
            visible_range: 0..len,
            hidden_count: 0,
            header,
            query: None,
            line_ending: "\n",
        },
    )?;

    Ok(())
}
