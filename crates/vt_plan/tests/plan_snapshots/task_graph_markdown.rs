use std::collections::BTreeMap;

use vt_graph::{IndexedTaskGraph, TaskGraphLoadError, TaskNode};
use vt_path::AbsolutePath;
use vt_str::Str;

use crate::redact::redact_snapshot_pretty_json;

type TaskGraphKey<'a> = (&'a AbsolutePath, &'a str);

struct TaskGraphEntry<'a> {
    key: TaskGraphKey<'a>,
    node: &'a TaskNode,
    neighbors: Vec<TaskGraphKey<'a>>,
}

fn task_graph_key(node: &TaskNode) -> TaskGraphKey<'_> {
    (node.task_display.package_path.as_ref(), node.task_display.task_name.as_str())
}

async fn sorted_task_graph_entries<'a>(
    task_graph: &'a IndexedTaskGraph<'_>,
) -> Result<Vec<TaskGraphEntry<'a>>, TaskGraphLoadError> {
    let mut entries = Vec::new();
    for node in task_graph.tasks().await? {
        let mut neighbors =
            task_graph.depends_on(&node.id).await?.map(task_graph_key).collect::<Vec<_>>();
        neighbors.sort_unstable();
        entries.push(TaskGraphEntry { key: task_graph_key(node), node, neighbors });
    }
    // Sort by the visible task identity so node IDs stay stable and snapshot
    // diffs don't depend on load order.
    entries.sort_unstable_by(|a, b| a.key.cmp(&b.key));
    Ok(entries)
}

fn redacted_package_path(path: &AbsolutePath, workspace_root: &AbsolutePath) -> Str {
    let relative = path
        .strip_prefix(workspace_root)
        .expect("package path must strip cleanly from workspace root")
        .expect("package path must be under workspace root");
    if relative.as_str().is_empty() {
        Str::from("<workspace>/")
    } else {
        vt_str::format!("<workspace>/{}", relative)
    }
}

fn task_graph_label(key: TaskGraphKey<'_>, workspace_root: &AbsolutePath) -> Str {
    let package_path = redacted_package_path(key.0, workspace_root);
    vt_str::format!("{package_path}#{}", key.1)
}

#[expect(
    clippy::disallowed_types,
    reason = "Mermaid label rendering needs a mutable string buffer"
)]
fn push_mermaid_label_text(out: &mut String, text: &str) {
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("#quot;"),
            _ => out.push(ch),
        }
    }
}

#[expect(
    clippy::disallowed_types,
    reason = "Markdown snapshot rendering needs a mutable string buffer"
)]
pub async fn render_task_graph_markdown(
    task_graph: &IndexedTaskGraph<'_>,
    workspace_root: &AbsolutePath,
) -> Result<String, TaskGraphLoadError> {
    let entries = sorted_task_graph_entries(task_graph).await?;
    let mut ids_by_key = BTreeMap::<TaskGraphKey<'_>, Str>::new();

    for (index, entry) in entries.iter().enumerate() {
        ids_by_key.insert(entry.key, vt_str::format!("task_{index}"));
    }

    let mut out = String::from("# task graph\n\n```mermaid\nflowchart TD\n");

    for entry in &entries {
        let node_id = ids_by_key.get(&entry.key).expect("task graph entry key must have a node id");
        let label = task_graph_label(entry.key, workspace_root);

        out.push_str("  ");
        out.push_str(node_id.as_str());
        out.push_str("[\"");
        push_mermaid_label_text(&mut out, label.as_str());
        out.push_str("\"]\n");

        for neighbor in &entry.neighbors {
            let target_id =
                ids_by_key.get(neighbor).expect("task graph neighbor key must have a node id");
            out.push_str("  ");
            out.push_str(node_id.as_str());
            out.push_str(" --> ");
            out.push_str(target_id.as_str());
            out.push('\n');
        }
    }

    out.push_str("```\n\n");

    for entry in &entries {
        // The heading identifies the task, and the Mermaid arrows capture
        // neighbors, so the detail block only needs the task node payload.
        let label = task_graph_label(entry.key, workspace_root);
        let detail_json = redact_snapshot_pretty_json(entry.node, workspace_root);
        out.push_str("## `");
        out.push_str(label.as_str());
        out.push_str("`\n\n```json\n");
        out.push_str(detail_json.as_str());
        out.push_str("\n```\n\n");
    }

    Ok(out)
}
