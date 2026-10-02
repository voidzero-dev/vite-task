//! Rendering for `vp run --dry-run`: one line per task with its predicted
//! cache result, in the style of the full run summary.

use owo_colors::Style;
use vt_path::AbsolutePath;
use vt_plan::ExecutionItemDisplay;
use vt_str::Str;

use super::{
    CACHE_MISS_STYLE, COMMAND_STYLE, ColorizeExt, format_command_display,
    summary::{SavedCacheMissReason, format_hit},
};
use crate::session::cache::CacheHitSource;

/// What a run would do with the cache for one task.
pub enum DryRunPrediction {
    /// The local cache has a valid entry.
    Hit,
    /// The local cache has no valid entry.
    Miss(SavedCacheMissReason),
    /// Caching is off for the task.
    Disabled,
    /// A built-in command, which is never cached.
    BuiltIn,
    /// A task that runs before this one isn't a cache hit, and running it
    /// could change this task's inputs. Holds that task's name or command.
    Unknown { runs_after: Str },
}

/// Format one task's line, e.g. `build: $ tsc → Cache hit`.
pub fn format_dry_run_line(
    display: &ExecutionItemDisplay,
    workspace_path: &AbsolutePath,
    prediction: &DryRunPrediction,
) -> Str {
    let (detail, style) = match prediction {
        DryRunPrediction::Hit => {
            (vt_str::format!("→ {}", format_hit(CacheHitSource::Local)), Style::new().green())
        }
        DryRunPrediction::Miss(reason) => {
            (vt_str::format!("→ Cache miss: {reason}"), CACHE_MISS_STYLE)
        }
        DryRunPrediction::Disabled => (Str::from("→ Cache disabled"), Style::new().bright_black()),
        DryRunPrediction::BuiltIn => {
            (Str::from("→ Cache disabled for built-in command"), Style::new().bright_black())
        }
        DryRunPrediction::Unknown { runs_after } => (
            vt_str::format!("→ Unknown: runs after '{runs_after}', which isn't a cache hit"),
            Style::new().yellow(),
        ),
    };
    vt_str::format!(
        "{}: {} {}\n",
        vt_str::format!("{}", display.task_display).style(Style::new().bright_white().bold()),
        format_command_display(display, workspace_path).style(COMMAND_STYLE),
        detail.style(style),
    )
}

/// The note printed once when a task has a remote cache configured.
pub fn format_remote_not_checked() -> Str {
    vt_str::format!(
        "{}\n",
        "Remote cache not checked: --dry-run only reads the local cache"
            .style(Style::new().bright_black())
    )
}
