//! What's printed below the tree when the run is over.

use std::io::Write as _;

use ratatui_core::style::{Modifier, Style};
use unicode_width::UnicodeWidthStr as _;

use crate::{
    ansi::write_style,
    tree::{TaskState, Tone},
    tui::Tui,
};

/// Header rules end at this column, or at the edge of a narrower terminal.
const REPORT_WIDTH: usize = 80;

impl Tui {
    /// Writes the output of each process in tree order: all of them, or only
    /// those that failed or warned. Each starts with a header line. Without
    /// decorations, the header has no rule and the output no colors.
    pub(crate) fn write_report(&self, out: &mut Vec<u8>, all: bool) {
        for task in self.tree.leaves() {
            let effective = self.tree.effective(task);
            let problem = effective.state == TaskState::Failed
                || (effective.tone == Tone::Warning && effective.state.is_finished());
            let ran = !matches!(
                effective.state,
                TaskState::NotRun | TaskState::Waiting | TaskState::Queued
            );
            if !(problem || (all && ran)) {
                continue;
            }
            let (icon, icon_style) = self.icon(effective);
            let title = self.title(task);
            out.extend_from_slice(b"\r\n");
            if self.options.decorations {
                write_style(out, icon_style);
                out.extend_from_slice(icon.as_bytes());
                write_style(out, Style::new().add_modifier(Modifier::BOLD));
                let _ = write!(out, " {title} ");
                let used = title.width() + 3;
                let rule =
                    usize::from(self.options.cols).min(REPORT_WIDTH).saturating_sub(used).max(3);
                write_style(out, Style::new().add_modifier(Modifier::DIM));
                for _ in 0..rule {
                    out.extend_from_slice("─".as_bytes());
                }
                out.extend_from_slice(b"\x1b[0m\r\n");
            } else {
                let _ = write!(out, "{icon} {title}\r\n");
            }
            if let Some(Some(emulator)) = self.emulators.get(task.index()) {
                emulator.write_text(out, self.options.decorations);
            }
        }
    }
}
