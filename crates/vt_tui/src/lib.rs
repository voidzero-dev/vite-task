//! The terminal UI of `vp run`: a live tree of the run's tasks in the primary
//! screen, and a full-screen view of one task's output with scrollback,
//! search and selection.
//!
//! The crate does no I/O. The caller builds a [`TaskTree`], creates a [`Tui`],
//! and then feeds it the run's events ([`Tui::set_state`], [`Tui::set_status`],
//! [`Tui::task_output`]), the terminal's input ([`Tui::input`]) and the time
//! ([`Tui::tick`]). It writes what [`Tui::render`] returns to the terminal, and
//! acts on [`Tui::take_requests`], such as sending a typed line to a task.
//! Where the terminal's input isn't a byte stream (Windows), the caller reads
//! events itself and passes them to [`Tui::input_event`].

mod ansi;
mod emulator;
mod input;
mod report;
mod task_view;
mod tree;
mod tree_view;
mod tui;

/// A key press, mouse event, paste or resize, for [`Tui::input_event`].
pub use terminput::Event as InputEvent;
pub use tree::{NodeId, NodeSpec, TaskState, TaskTree, Tone};
pub use tui::{Options, Request, Tui};

#[cfg(test)]
mod tests;
