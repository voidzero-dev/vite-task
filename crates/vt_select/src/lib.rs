mod fuzzy;
mod interactive;

use std::io::Write;

pub use fuzzy::fuzzy_match;
use interactive::{build_display_rows, render_non_interactive_items};
use vt_str::Str;

/// An item in the selection list.
pub struct SelectItem {
    /// Searchable label, e.g. `"build"` or `"app#build"`. Used for fuzzy matching.
    pub label: Str,
    /// Display name shown in the list, e.g. `"build"` (tree view) or `"app#build"` (flat).
    pub display_name: Str,
    /// Description shown next to the display name, e.g. `"echo build app"`.
    pub description: Str,
    /// Group header text. Items sharing the same group render together under a
    /// header line. `None` = top-level (no header).
    pub group: Option<Str>,
}

/// Snapshot of the selector's visible state, passed to `after_render`.
pub struct RenderState<'a> {
    /// Current search text (empty if no filter typed yet).
    pub query: &'a str,
    /// Index of the highlighted item in the **filtered** list.
    pub selected_index: usize,
}

/// Parameters for [`select_interactive`].
pub struct SelectParams<'a> {
    pub items: &'a [SelectItem],
    /// Initial search query pre-filled in the interactive filter.
    pub query: Option<&'a str>,
    /// Header line rendered above the list (e.g. an error message).
    pub header: Option<&'a str>,
    /// Prompt line rendered above the list.
    pub prompt: &'a str,
    /// Max visible rows in the viewport.
    pub page_size: usize,
}

/// Parameters for [`print_list`].
pub struct PrintListParams<'a> {
    pub items: &'a [SelectItem],
    /// Optional search query used to filter items before printing.
    pub query: Option<&'a str>,
    /// Header line rendered above the list (e.g. an error message).
    pub header: Option<&'a str>,
}

/// Outcome of an interactive selection.
pub enum SelectOutcome {
    /// The user selected the item at this index in the original `items` slice.
    Selected(usize),
    /// The user cancelled the selection (e.g. Ctrl+C).
    Cancelled,
}

/// Show an interactive task selection list in the terminal.
///
/// Enters raw mode with fuzzy search and keyboard navigation. `after_render` is
/// called after every render with the current visible state (useful for emitting
/// test milestones). On Enter, returns [`SelectOutcome::Selected`] with the
/// chosen item's index in the original `items` slice. Returns
/// [`SelectOutcome::Cancelled`] if the user presses Ctrl+C.
///
/// # Errors
///
/// Returns an error if `items` is empty or terminal I/O fails.
pub fn select_interactive(
    params: &SelectParams<'_>,
    after_render: impl FnMut(&RenderState<'_>),
) -> anyhow::Result<SelectOutcome> {
    interactive::run(params, after_render)
}

/// Print a non-interactive task list once to `writer`.
///
/// Items are filtered by `params.query` (if any) and grouped so current-package
/// tasks appear first, rendered flat without group headers or truncation. When
/// no items match, only `params.header` (if any) is written.
///
/// # Errors
///
/// Returns an error if writing to `writer` fails.
pub fn print_list(writer: &mut impl Write, params: &PrintListParams<'_>) -> anyhow::Result<()> {
    let display_rows = build_display_rows(params.items, params.query.unwrap_or_default());
    render_non_interactive_items(writer, params.items, &display_rows, params.header)
}
