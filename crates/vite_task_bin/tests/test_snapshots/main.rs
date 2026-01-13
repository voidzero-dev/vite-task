mod redact;

use std::{
    collections::HashMap,
    env::{self, join_paths, split_paths},
    ffi::OsStr,
    path::Path,
    process::Command,
    sync::Arc,
};

use copy_dir::copy_dir;
use redact::{redact_e2e_output, redact_snapshot};
use tokio::runtime::Runtime;
use vite_path::{AbsolutePath, AbsolutePathBuf, RelativePathBuf};
use vite_str::Str;
use vite_task::Session;
use vite_workspace::find_workspace_root;

#[derive(serde::Deserialize, Debug)]
struct E2e {
    pub name: Str,
    #[serde(default)]
    pub cwd: RelativePathBuf,
    pub steps: Vec<Str>,
}

#[derive(serde::Deserialize, Default)]
struct SnapshotsFile {
    #[serde(rename = "e2e", default)] // toml usually uses singular for arrays
    pub e2e_cases: Vec<E2e>,
}

fn run_case(runtime: &Runtime, tmpdir: &AbsolutePath, fixture_path: &Path) {
    let fixture_name = fixture_path.file_name().unwrap().to_str().unwrap();
    if fixture_name.starts_with(".") {
        return; // skip hidden files like .DS_Store
    }

    // Configure insta to write snapshots to fixture directory
    let mut settings = insta::Settings::clone_current();
    settings.set_snapshot_path(fixture_path.join("snapshots"));
    settings.set_prepend_module_to_snapshot(false);
    settings.remove_snapshot_suffix();

    settings.bind(|| run_case_inner(runtime, tmpdir, fixture_path, fixture_name));
}

fn run_case_inner(
    runtime: &Runtime,
    tmpdir: &AbsolutePath,
    fixture_path: &Path,
    fixture_name: &str,
) {
    // Copy the case directory to a temporary directory to avoid discovering workspace outside of the test case.
    let stage_path = tmpdir.join(fixture_name);
    copy_dir(fixture_path, &stage_path).unwrap();

    let (workspace_root, _cwd) = find_workspace_root(&stage_path).unwrap();

    assert_eq!(
        &stage_path, &*workspace_root.path,
        "folder '{}' should be a workspace root",
        fixture_name
    );

    let cases_toml_path = fixture_path.join("snapshots.toml");
    let cases_file: SnapshotsFile = match std::fs::read(&cases_toml_path) {
        Ok(content) => toml::from_slice(&content).unwrap(),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Default::default(),
        Err(err) => panic!("Failed to read cases.toml for fixture {}: {}", fixture_name, err),
    };

    // Navigate from CARGO_MANIFEST_DIR to packages/tools at the repo root
    let repo_root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap();
    let test_bin_path = Arc::<OsStr>::from(
        repo_root.join("packages").join("tools").join("node_modules").join(".bin").into_os_string(),
    );

    // Find @yarnpkg/shell executable in packages/tools
    let shell_exe =
        which::which_in("shell", Some(&*test_bin_path), std::env::current_dir().unwrap())
            .expect("shell executable not found in packages/tools/node_modules/.bin");

    // Add packages/tools to PATH so test programs (such as print-file) in fixtures can be found.
    let plan_envs: HashMap<Arc<OsStr>, Arc<OsStr>> = [
        (Arc::<OsStr>::from(OsStr::new("PATH")), Arc::clone(&test_bin_path)),
        (Arc::<OsStr>::from(OsStr::new("NO_COLOR")), Arc::<OsStr>::from(OsStr::new("1"))),
    ]
    .into_iter()
    .collect();

    // Prepare PATH for e2e tests
    let e2e_env_path = join_paths(
        [
            // Include vite binary path to PATH so that e2e tests can run "vite ..." commands.
            {
                let vite_path = AbsolutePath::new(env!("CARGO_BIN_EXE_vite")).unwrap();
                let vite_dir = vite_path.parent().unwrap();
                vite_dir.as_path().as_os_str().into()
            },
            // Include packages/tools to PATH so that e2e tests can run utilities such as json-edit.
            test_bin_path,
        ]
        .into_iter()
        .chain(
            // the existing PATH
            split_paths(&env::var_os("PATH").unwrap())
                .map(|path| Arc::<OsStr>::from(path.into_os_string())),
        ),
    )
    .unwrap();

    runtime.block_on(async {
        let workspace_root_str = workspace_root.path.as_path().to_str().unwrap();
        let mut owned_callbacks = vite_task_bin::OwnedSessionCallbacks::default();
        let mut session = Session::init_with(
            plan_envs.into(),
            Arc::clone(&workspace_root.path),
            owned_callbacks.as_callbacks(),
        )
        .unwrap();

        // Validate task graph loads correctly for the fixture
        let task_graph_json = redact_snapshot(
            &vite_graph_ser::SerializeByKey(
                session.ensure_task_graph_loaded().await.unwrap().task_graph(),
            ),
            workspace_root_str,
        );
        insta::assert_json_snapshot!("task graph", task_graph_json);

        let mut e2e_count = 0u32;
        for e2e in cases_file.e2e_cases {
            let e2e_stage_path = tmpdir.join(format!("{}_e2e_stage_{}", fixture_name, e2e_count));
            e2e_count += 1;
            assert!(copy_dir(fixture_path, &e2e_stage_path).unwrap().is_empty());

            let e2e_stage_path_str = e2e_stage_path.as_path().to_str().unwrap();

            let mut e2e_outputs = String::new();
            for step in e2e.steps {
                // Use @yarnpkg/shell for cross-platform shell execution
                let mut cmd = Command::new(&shell_exe);
                cmd.arg(step.as_str())
                    .env_clear()
                    .env("PATH", &e2e_env_path)
                    .env("NO_COLOR", "1")
                    .current_dir(e2e_stage_path.join(&e2e.cwd));
                let output = cmd.output().unwrap();

                let exit_code = output.status.code().unwrap_or(-1);
                if exit_code != 0 {
                    e2e_outputs.push_str(format!("[{}]", exit_code).as_str());
                }
                e2e_outputs.push_str("> ");
                e2e_outputs.push_str(step.as_str());
                e2e_outputs.push('\n');

                let stdout = String::from_utf8(output.stdout).unwrap();
                let stderr = String::from_utf8(output.stderr).unwrap();
                e2e_outputs.push_str(&redact_e2e_output(stdout, e2e_stage_path_str));
                e2e_outputs.push_str(&redact_e2e_output(stderr, e2e_stage_path_str));
                e2e_outputs.push('\n');
            }
            insta::assert_snapshot!(e2e.name.as_str(), e2e_outputs);
        }
    });
}

#[test]
fn test_snapshots() {
    let tokio_runtime = Runtime::new().unwrap();
    let tmp_dir = tempfile::tempdir().unwrap();
    let tmp_dir_path = AbsolutePathBuf::new(tmp_dir.path().canonicalize().unwrap()).unwrap();

    let tests_dir = std::env::current_dir().unwrap().join("tests");

    insta::glob!(tests_dir, "test_snapshots/fixtures/*", |case_path| run_case(
        &tokio_runtime,
        &tmp_dir_path,
        case_path
    ));
}
