mod redact;

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    sync::Arc,
};

use clap::Parser;
use copy_dir::copy_dir;
use cow_utils::CowUtils as _;
use redact::redact_snapshot;
use rustc_hash::FxHashMap;
use serde::Serialize;
use tokio::runtime::Runtime;
use vite_path::{AbsolutePath, AbsolutePathBuf, RelativePathBuf};
use vite_str::Str;
use vite_task::{Command, Session};
use vite_task_graph::display::TaskDisplay;
use vite_task_plan::{ExecutionGraph, ExecutionItemKind};
use vite_workspace::find_workspace_root;

/// Local parser wrapper for `BuiltInCommand`
#[derive(Parser)]
#[command(name = "vt")]
enum Cli {
    #[clap(flatten)]
    Command(Command),
}

#[derive(serde::Deserialize, Debug)]
struct Plan {
    pub name: Str,
    pub args: Vec<Str>,
    #[serde(default)]
    pub cwd: RelativePathBuf,
    #[serde(default)]
    pub compact: bool,
    #[serde(default)]
    pub env: BTreeMap<Str, Str>,
}

#[derive(serde::Deserialize, Default)]
struct SnapshotsFile {
    /// Optional platform filter: `"unix"` or `"windows"`. If set, the whole
    /// fixture only runs on that platform. Fixtures whose filter doesn't
    /// match the current build are dropped during trial enumeration so they
    /// don't show up as "passed" when they never ran. Mirrors the per-case
    /// filter used by the e2e harness.
    #[serde(default)]
    pub platform: Option<Str>,
    #[serde(rename = "plan", default)] // toml usually uses singular for arrays
    pub plan_cases: Vec<Plan>,
}

/// Returns whether the current build should run the fixture. Panics on an
/// unknown platform string so typos surface loudly.
fn should_run_on_this_platform(platform: Option<&Str>) -> bool {
    match platform.map(Str::as_str) {
        None => true,
        Some("unix") => cfg!(unix),
        Some("windows") => cfg!(windows),
        Some(other) => {
            panic!("Unknown platform filter '{other}' — expected \"unix\" or \"windows\"")
        }
    }
}

/// Compact plan: maps `"relative_path#task_name"` to either just neighbors (simple)
/// or `{ items, neighbors }` when the node has nested `Expanded` execution items.
#[derive(Serialize)]
#[serde(transparent)]
struct CompactPlan(BTreeMap<Str, CompactNode>);

/// Untagged enum so simple nodes serialize as just an array, and nodes with
/// expanded items serialize as `{ "items": [...], "neighbors": [...] }`.
#[derive(Serialize)]
#[serde(untagged)]
enum CompactNode {
    /// No nested `Expanded` items — just the neighbor list
    Simple(BTreeSet<Str>),
    /// Has nested `Expanded` items
    WithItems { items: Vec<CompactPlan>, neighbors: BTreeSet<Str> },
}

impl CompactPlan {
    fn from_execution_graph(graph: &ExecutionGraph, workspace_root: &AbsolutePath) -> Self {
        use petgraph::visit::EdgeRef as _;
        let mut map = BTreeMap::<Str, CompactNode>::new();
        for node_idx in graph.graph.node_indices() {
            let node = &graph.graph[node_idx];
            let key = Self::task_key(&node.task_display, workspace_root);

            let neighbors: BTreeSet<Str> = graph
                .graph
                .edges(node_idx)
                .map(|edge| {
                    Self::task_key(&graph.graph[edge.target()].task_display, workspace_root)
                })
                .collect();

            let expanded_items: Vec<Self> = node
                .items
                .iter()
                .filter_map(|item| {
                    if let ExecutionItemKind::Expanded(sub_graph) = &item.kind {
                        Some(Self::from_execution_graph(sub_graph, workspace_root))
                    } else {
                        None
                    }
                })
                .collect();

            let compact_node = if expanded_items.is_empty() {
                CompactNode::Simple(neighbors)
            } else {
                CompactNode::WithItems { items: expanded_items, neighbors }
            };
            map.insert(key, compact_node);
        }
        Self(map)
    }

    fn task_key(task_display: &TaskDisplay, workspace_root: &AbsolutePath) -> Str {
        let relative = task_display
            .package_path
            .strip_prefix(workspace_root)
            .expect("strip_prefix should not produce invalid path data")
            .expect("package_path must be under workspace_root");
        vite_str::format!("{}#{}", relative, task_display.task_name)
    }
}

/// Fixture folder names and `[[plan]].name` values must be made of
/// `[A-Za-z0-9_]` only so trial names round-trip through shell filters
/// and snapshot filenames don't carry whitespace or special characters.
fn assert_identifier_like(kind: &str, value: &str) {
    assert!(
        !value.is_empty() && value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'),
        "{kind} '{value}' must contain only ASCII letters, digits, and '_'"
    );
}

#[expect(
    clippy::disallowed_types,
    reason = "Path required for fixture handling; String required by std::fs::read and toml::from_slice"
)]
fn load_snapshots_file(fixture_path: &std::path::Path, fixture_name: &str) -> SnapshotsFile {
    let cases_toml_path = fixture_path.join("snapshots.toml");
    match std::fs::read(&cases_toml_path) {
        Ok(content) => toml::from_slice(&content).unwrap_or_else(|err| {
            panic!("Failed to parse snapshots.toml for fixture {fixture_name}: {err}")
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => SnapshotsFile::default(),
        Err(err) => panic!("Failed to read snapshots.toml for fixture {fixture_name}: {err}"),
    }
}

#[expect(clippy::disallowed_types, reason = "Path required for fixture path handling")]
fn run_case(
    runtime: &Runtime,
    tmpdir: &AbsolutePath,
    fixture_path: &std::path::Path,
    cases_file: SnapshotsFile,
) -> Result<(), String> {
    let fixture_name = fixture_path.file_name().unwrap().to_str().unwrap();
    assert_identifier_like("fixture folder", fixture_name);
    let snapshots = snapshot_test::Snapshots::new(fixture_path.join("snapshots"));
    run_case_inner(runtime, tmpdir, fixture_path, fixture_name, &snapshots, cases_file)
}

#[expect(
    clippy::disallowed_types,
    reason = "Path required for fixture handling; String required by std::fs::read and toml::from_slice"
)]
#[expect(clippy::too_many_lines, reason = "test setup and assertion logic in a single function")]
fn run_case_inner(
    runtime: &Runtime,
    tmpdir: &AbsolutePath,
    fixture_path: &std::path::Path,
    fixture_name: &str,
    snapshots: &snapshot_test::Snapshots,
    cases_file: SnapshotsFile,
) -> Result<(), String> {
    // Copy the case directory to a temporary directory to avoid discovering workspace outside of the test case.
    let stage_path = tmpdir.join(fixture_name);
    copy_dir(fixture_path, &stage_path).unwrap();

    let (workspace_root, _cwd) = find_workspace_root(&stage_path).unwrap();

    assert_eq!(
        &stage_path, &*workspace_root.path,
        "folder '{fixture_name}' should be a workspace root"
    );

    let fake_bin_dir = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("tests/plan_snapshots/fake-bin");
    let combined_path =
        Arc::<OsStr>::from(std::ffi::OsString::from(fake_bin_dir.to_str().unwrap()));

    let plan_envs: FxHashMap<Arc<OsStr>, Arc<OsStr>> = [
        (Arc::<OsStr>::from(OsStr::new("PATH")), combined_path),
        (Arc::<OsStr>::from(OsStr::new("NO_COLOR")), Arc::<OsStr>::from(OsStr::new("1"))),
    ]
    .into_iter()
    .collect();

    runtime.block_on(async {
        let workspace_root_str = workspace_root.path.as_path().to_str().unwrap();
        let mut owned_config = vite_task_bin::OwnedSessionConfig::default();
        let mut session = Session::init_with(
            plan_envs.clone(),
            Arc::clone(&workspace_root.path),
            owned_config.as_config(),
        )
        .unwrap();

        let task_graph_result = session.ensure_task_graph_loaded().await;
        let task_graph = match task_graph_result {
            Ok(task_graph) => task_graph,
            Err(err) => {
                let err_formatted = vite_str::format!("{err:#}");
                let err_str = err_formatted.as_str().cow_replace(workspace_root_str, "<workspace>");
                let err_str =
                    if cfg!(windows) { err_str.as_ref().cow_replace('\\', "/") } else { err_str };
                snapshots.check_snapshot("task_graph_load_error.snap", err_str.as_ref())?;
                return Ok(());
            }
        };
        let task_graph_json = redact_snapshot(
            &vite_graph_ser::SerializeByKey(task_graph.task_graph()),
            workspace_root_str,
        );
        snapshots.check_json_snapshot("task_graph", "task graph", &task_graph_json)?;

        for plan in cases_file.plan_cases {
            assert_identifier_like("plan case name", plan.name.as_str());
            let snapshot_base = vite_str::format!("query_{}", plan.name);
            let compact = plan.compact;
            let args_display =
                plan.args.iter().map(vite_str::Str::as_str).collect::<Vec<_>>().join(" ");

            let cli = match Cli::try_parse_from(
                std::iter::once("vt") // dummy program name
                    .chain(plan.args.iter().map(vite_str::Str::as_str)),
            ) {
                Ok(ok) => ok,
                Err(err) => {
                    snapshots.check_snapshot(
                        vite_str::format!("{snapshot_base}.snap").as_str(),
                        &err.to_string(),
                    )?;
                    continue;
                }
            };
            let Cli::Command(parsed) = cli;
            let Command::Run(run_command) = parsed else {
                panic!("only `run` commands supported in plan tests")
            };

            // Create a fresh session per plan case with case-specific env vars and cwd.
            let mut case_envs = plan_envs.clone();
            for (k, v) in &plan.env {
                case_envs
                    .insert(Arc::from(OsStr::new(k.as_str())), Arc::from(OsStr::new(v.as_str())));
            }
            let case_cwd: Arc<AbsolutePath> = workspace_root.path.join(plan.cwd).into();
            let mut case_owned_config = vite_task_bin::OwnedSessionConfig::default();
            let mut case_session =
                Session::init_with(case_envs, Arc::clone(&case_cwd), case_owned_config.as_config())
                    .unwrap();
            case_session.ensure_task_graph_loaded().await.unwrap();

            let plan_result = case_session.plan_from_cli_run(case_cwd, run_command).await;

            let plan = match plan_result {
                Ok(graph) => graph,
                Err(err) => {
                    // Format the full error chain using anyhow's `{:#}` formatter
                    // and redact workspace paths for snapshot stability.
                    let anyhow_err: anyhow::Error = err.into();
                    let err_formatted = vite_str::format!("{anyhow_err:#}");
                    let err_str =
                        err_formatted.as_str().cow_replace(workspace_root_str, "<workspace>");
                    let err_str = if cfg!(windows) {
                        err_str.as_ref().cow_replace('\\', "/")
                    } else {
                        err_str
                    };
                    snapshots.check_snapshot(
                        vite_str::format!("{snapshot_base}.snap").as_str(),
                        err_str.as_ref(),
                    )?;
                    continue;
                }
            };

            let comment = vite_str::format!("{args_display}");
            if compact {
                let compact_plan = CompactPlan::from_execution_graph(&plan, &workspace_root.path);
                snapshots.check_json_snapshot(
                    snapshot_base.as_str(),
                    comment.as_str(),
                    &compact_plan,
                )?;
            } else {
                let plan_json = redact_snapshot(&plan, workspace_root_str);
                snapshots.check_json_snapshot(
                    snapshot_base.as_str(),
                    comment.as_str(),
                    &plan_json,
                )?;
            }
        }
        Ok(())
    })
}

#[expect(clippy::disallowed_types, reason = "Path required for CARGO_MANIFEST_DIR path traversal")]
fn main() {
    let tokio_runtime = Arc::new(Runtime::new().unwrap());
    let tmp_dir = tempfile::tempdir().unwrap();
    let tmp_dir_path = AbsolutePathBuf::new(tmp_dir.path().canonicalize().unwrap()).unwrap();

    let fixtures_dir = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("tests/plan_snapshots/fixtures");

    let mut fixture_paths = std::fs::read_dir(fixtures_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| !n.starts_with('.')))
        .collect::<Vec<_>>();
    fixture_paths.sort();

    let args = libtest_mimic::Arguments::from_args();

    let tests: Vec<libtest_mimic::Trial> = fixture_paths
        .into_iter()
        .filter_map(|fixture_path| {
            // Parse `snapshots.toml` once. Fixtures whose platform filter
            // doesn't match the current build are dropped entirely — if we
            // early-returned from the test body instead they'd report as
            // "passed" without having run.
            let fixture_name = fixture_path.file_name().unwrap().to_str().unwrap().to_owned();
            let cases_file = load_snapshots_file(&fixture_path, &fixture_name);
            if !should_run_on_this_platform(cases_file.platform.as_ref()) {
                return None;
            }

            let tmp_dir_path = tmp_dir_path.clone();
            let runtime = Arc::clone(&tokio_runtime);
            Some(libtest_mimic::Trial::test(fixture_name, move || {
                run_case(&runtime, &tmp_dir_path, &fixture_path, cases_file).map_err(Into::into)
            }))
        })
        .collect();

    libtest_mimic::run(&args, tests).exit();
}
