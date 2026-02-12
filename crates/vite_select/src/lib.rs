mod fuzzy;
mod interactive;

use std::io::Write;

pub use fuzzy::fuzzy_match;
use vite_str::Str;

/// An item in the selection list.
pub struct SelectItem {
    /// Display label, e.g. `"build"` or `"app#build"`.
    pub label: Str,
    /// Description shown next to the label, e.g. `"echo build app"`.
    pub description: Str,
}

/// Snapshot of the selector's visible state, passed to `after_render`.
pub struct RenderState<'a> {
    /// Current search text (empty if no filter typed yet).
    pub query: &'a str,
    /// Index of the highlighted item in the **filtered** list.
    pub selected_index: usize,
}

/// Result returned when the user confirms a selection.
pub struct SelectResult {
    /// Index into the *original* `items` slice.
    pub original_index: usize,
}

/// Show an interactive fuzzy-searchable select list.
///
/// `after_render` is called after every render with the current visible state
/// (useful for emitting test milestones).
///
/// Returns `Ok(None)` if the user cancels (Esc / Ctrl-C).
///
/// # Errors
///
/// Returns an error if terminal I/O fails.
pub fn interactive_select(
    items: &[SelectItem],
    initial_query: Option<&str>,
    header: Option<&str>,
    page_size: usize,
    after_render: impl FnMut(&RenderState<'_>),
) -> anyhow::Result<Option<SelectResult>> {
    interactive::run(items, initial_query, header, page_size, after_render)
}

/// Print a list of items to `writer` (non-interactive).
///
/// When `query` is `Some(q)`, only items matching the fuzzy filter are printed.
/// When `query` is `None`, all items are printed.
///
/// `header` is printed above the list (e.g. an error message).
///
/// # Errors
///
/// Returns an error if writing fails.
pub fn print_select_list(
    writer: &mut impl Write,
    items: &[SelectItem],
    query: Option<&str>,
    header: Option<&str>,
) -> anyhow::Result<()> {
    if let Some(header) = header {
        writeln!(writer, "{header}")?;
    }

    let labels: Vec<&str> = items.iter().map(|item| item.label.as_str()).collect();

    let indices: Vec<usize> =
        query.map_or_else(|| (0..items.len()).collect(), |q| fuzzy_match(q, &labels));

    if indices.is_empty() {
        writeln!(writer, "  No matching tasks found.")?;
        return Ok(());
    }

    for &idx in &indices {
        let item = &items[idx];
        writeln!(writer, "  {}: {}", item.label, item.description)?;
    }

    Ok(())
}
