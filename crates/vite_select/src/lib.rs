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

/// Parameters for [`select_list`].
pub struct SelectParams<'a> {
    pub items: &'a [SelectItem],
    /// Initial search query (pre-filled in interactive, used as filter in non-interactive).
    pub query: Option<&'a str>,
    /// Header line rendered above the list (e.g. an error message).
    pub header: Option<&'a str>,
    /// Max visible rows (interactive only).
    pub page_size: usize,
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
    params: &SelectParams<'_>,
    mode: Mode<'_>,
    before_render: impl FnMut(&mut Vec<usize>, &str),
    after_render: impl FnMut(&RenderState<'_>),
) -> anyhow::Result<()> {
    match mode {
        Mode::Interactive { selected_index } => interactive::run(
            params.items,
            params.query,
            selected_index,
            params.header,
            params.page_size,
            before_render,
            after_render,
        ),
        Mode::NonInteractive => {
            non_interactive(writer, params.items, params.query, params.header, before_render)
        }
    }
}

fn non_interactive(
    writer: &mut impl Write,
    items: &[SelectItem],
    query: Option<&str>,
    header: Option<&str>,
    mut before_render: impl FnMut(&mut Vec<usize>, &str),
) -> anyhow::Result<()> {
    let labels: Vec<&str> = items.iter().map(|item| item.label.as_str()).collect();
    let mut filtered: Vec<usize> =
        query.map_or_else(|| (0..items.len()).collect(), |q| fuzzy_match(q, &labels));
    before_render(&mut filtered, query.unwrap_or_default());
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
            max_line_width: usize::MAX,
        },
    )?;

    Ok(())
}
