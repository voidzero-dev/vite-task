use std::collections::BTreeMap;

use petgraph::visit::{EdgeRef as _, IntoNodeReferences};
use vite_path::AbsolutePath;
use vite_str::Str;
use vite_task_graph::{TaskGraph, TaskNode};

use crate::redact::redact_snapshot_pretty_json;

type TaskGraphKey<'a> = (&'a AbsolutePath, &'a str);

struct TaskGraphEntry<'a> {
    key: TaskGraphKey<'a>,
    node: &'a TaskNode,
    neighbors: Vec<TaskGraphKey<'a>>,
}

impl<'a> TaskGraphEntry<'a> {
    fn from_graph_node(
        task_graph: &'a TaskGraph,
        node_index: vite_task_graph::TaskNodeIndex,
        node: &'a TaskNode,
    ) -> Self {
        let mut neighbors = task_graph
            .edges(node_index)
            .map(|edge| task_graph_key(&task_graph[edge.target()]))
            .collect::<Vec<_>>();
        neighbors.sort_unstable();
        Self { key: task_graph_key(node), node, neighbors }
    }
}

fn task_graph_key(node: &TaskNode) -> TaskGraphKey<'_> {
    (node.task_display.package_path.as_ref(), node.task_display.task_name.as_str())
}

fn sorted_task_graph_entries(task_graph: &TaskGraph) -> Vec<TaskGraphEntry<'_>> {
    // Sort by the visible task identity so node IDs stay stable and snapshot
    // diffs don't depend on petgraph insertion order.
    let mut entries = task_graph
        .node_references()
        .map(|(node_index, node)| TaskGraphEntry::from_graph_node(task_graph, node_index, node))
        .collect::<Vec<_>>();
    entries.sort_unstable_by(|a, b| a.key.cmp(&b.key));
    entries
}

fn redacted_package_path(path: &AbsolutePath, workspace_root: &AbsolutePath) -> Str {
    let relative = path
        .strip_prefix(workspace_root)
        .expect("package path must strip cleanly from workspace root")
        .expect("package path must be under workspace root");
    if relative.as_str().is_empty() {
        Str::from("<workspace>/")
    } else {
        vite_str::format!("<workspace>/{}", relative)
    }
}

fn task_graph_label(key: TaskGraphKey<'_>, workspace_root: &AbsolutePath) -> Str {
    let package_path = redacted_package_path(key.0, workspace_root);
    vite_str::format!("{package_path}#{}", key.1)
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
pub fn render_task_graph_markdown(task_graph: &TaskGraph, workspace_root: &AbsolutePath) -> String {
    let entries = sorted_task_graph_entries(task_graph);
    let mut ids_by_key = BTreeMap::<TaskGraphKey<'_>, Str>::new();

    for (index, entry) in entries.iter().enumerate() {
        ids_by_key.insert(entry.key, vite_str::format!("task_{index}"));
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

    out
}
