use std::{ffi::OsStr, sync::Arc};

use vt_graph::config::UserCacheConfig;
use vt_path::{AbsolutePath, AbsolutePathBuf};
use vt_plan::{SpawnExecution, plan_request::SyntheticPlanRequest, plan_synthetic};
use vt_str::Str;

fn root() -> Arc<AbsolutePath> {
    AbsolutePathBuf::new(std::env::temp_dir()).unwrap().join("tool-entry-workspace").into()
}

fn plan(
    workspace: &Arc<AbsolutePath>,
    cwd: &Arc<AbsolutePath>,
    entry: Option<Arc<AbsolutePath>>,
    args: Arc<[Str]>,
) -> SpawnExecution {
    let program = std::env::current_exe().unwrap().into_os_string().into();
    plan_synthetic(
        workspace,
        cwd,
        SyntheticPlanRequest {
            program,
            args,
            tool_entry: entry,
            cache_config: UserCacheConfig::Bool(true),
            envs: Arc::default(),
        },
        Arc::from([Str::from("tool")]),
    )
    .unwrap()
}

#[test]
fn tool_entry_uses_task_cwd_and_identical_spawn_and_cache_arguments() {
    let temp = tempfile::tempdir().unwrap();
    let workspace: Arc<AbsolutePath> =
        AbsolutePathBuf::new(temp.path().to_path_buf()).unwrap().into();
    let entry: Arc<AbsolutePath> = workspace.join("node_modules/tool/bin.js").into();
    std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
    std::fs::create_dir_all(workspace.join("packages/nested")).unwrap();
    std::fs::write(entry.as_path(), "tool entry").unwrap();
    for (cwd, expected) in [
        (Arc::clone(&workspace), "./node_modules/tool/bin.js"),
        (workspace.join("packages/nested").into(), "../../node_modules/tool/bin.js"),
    ] {
        let args: Arc<[Str]> = Arc::from([
            Str::from("--config"),
            Str::from(entry.as_path().to_str().unwrap()),
            Str::from("../untouched"),
        ]);
        let planned = plan(&workspace, &cwd, Some(Arc::clone(&entry)), Arc::clone(&args));
        assert_eq!(planned.spawn_command.cwd, cwd);
        assert_eq!(planned.spawn_command.args[0], expected);
        assert_eq!(
            std::fs::read_to_string(cwd.join(planned.spawn_command.args[0].as_str())).unwrap(),
            "tool entry"
        );
        assert_eq!(&planned.spawn_command.args[1..], args.as_ref());
        assert_eq!(
            planned.cache_metadata.unwrap().spawn_fingerprint.args().as_ref(),
            planned.spawn_command.args.as_ref()
        );
    }
}

#[test]
fn tool_entry_relocation_has_the_same_spawn_fingerprint() {
    let a = root();
    let b: Arc<AbsolutePath> = a.parent().unwrap().join("other-workspace").into();
    let fingerprints: Vec<_> = [Arc::clone(&a), b]
        .iter()
        .map(|workspace| {
            plan(
                workspace,
                &workspace.join("packages/nested").into(),
                Some(workspace.join("node_modules/tool/bin.js").into()),
                Arc::from([Str::from("run")]),
            )
            .cache_metadata
            .unwrap()
            .spawn_fingerprint
        })
        .collect();
    assert_eq!(fingerprints[0], fingerprints[1]);
    for (entry, args) in [
        ("node_modules/other/bin.js", Arc::from([Str::from("run")])),
        ("node_modules/tool/bin.js", Arc::from([Str::from("build")])),
    ] {
        let changed = plan(&a, &a.join("packages/nested").into(), Some(a.join(entry).into()), args);
        assert_ne!(fingerprints[0], changed.cache_metadata.unwrap().spawn_fingerprint);
    }
}

#[test]
fn tool_entry_preserves_external_and_traversal_paths() {
    let workspace = root();
    for entry in [
        workspace.parent().unwrap().join("tool-entry-workspace-other/bin.js"),
        workspace.join("../external/bin.js"),
        workspace.join("node_modules/link/../bin.js"),
    ] {
        let entry: Arc<AbsolutePath> = entry.into();
        let planned = plan(&workspace, &workspace, Some(Arc::clone(&entry)), Arc::default());
        assert_eq!(OsStr::new(planned.spawn_command.args[0].as_str()), entry.as_path());
    }
}

#[test]
fn tool_entry_absent_preserves_all_arguments() {
    let workspace = root();
    let args: Arc<[Str]> = Arc::from([
        Str::from(workspace.join("node_modules/tool/bin.js").as_path().to_str().unwrap()),
        Str::from("--entry=/absolute/opaque.js"),
    ]);
    let planned = plan(&workspace, &workspace, None, Arc::clone(&args));
    assert_eq!(planned.spawn_command.args, args);
}

#[test]
fn tool_entry_option_like_filename_stays_a_path() {
    let workspace = root();
    let planned =
        plan(&workspace, &workspace, Some(workspace.join("-tool.js").into()), Arc::default());
    assert_eq!(planned.spawn_command.args[0], "./-tool.js");
}

#[test]
fn ordinary_script_conversion_does_not_declare_a_tool_entry() {
    let command = vt_plan::plan_request::ScriptCommand {
        program: Str::from("node"),
        args: Arc::from([Str::from("/opaque/script.js")]),
        envs: Arc::default(),
        cwd: root(),
    };
    let request = command.to_synthetic_plan_request(UserCacheConfig::Bool(true));
    assert!(request.tool_entry.is_none());
    assert_eq!(request.args, command.args);
}

#[test]
fn non_entry_absolute_arguments_remain_location_sensitive() {
    let workspace = root();
    let fingerprints: Vec<_> = ["first", "second"]
        .iter()
        .map(|name| {
            let entry: Arc<AbsolutePath> = workspace.join(name).join("bin.js").into();
            plan(
                &workspace,
                &workspace,
                None,
                Arc::from([Str::from(entry.as_path().to_str().unwrap())]),
            )
            .cache_metadata
            .unwrap()
            .spawn_fingerprint
        })
        .collect();
    assert_ne!(fingerprints[0], fingerprints[1]);
}

#[test]
fn tool_entry_rejects_non_utf8_without_lossy_conversion() {
    #[cfg(unix)]
    let invalid = {
        use std::os::unix::ffi::OsStringExt as _;
        std::ffi::OsString::from_vec(vec![0xff])
    };
    #[cfg(windows)]
    let invalid = {
        use std::os::windows::ffi::OsStringExt as _;
        std::ffi::OsString::from_wide(&[0xd800])
    };
    let workspace = root();
    let entry: Arc<AbsolutePath> = workspace.join(invalid).into();
    for enabled in [false, true] {
        let result = plan_synthetic(
            &workspace,
            &workspace,
            SyntheticPlanRequest {
                program: std::env::current_exe().unwrap().into_os_string().into(),
                args: Arc::default(),
                tool_entry: Some(Arc::clone(&entry)),
                cache_config: UserCacheConfig::Bool(enabled),
                envs: Arc::default(),
            },
            Arc::from([Str::from("tool")]),
        );
        assert!(matches!(result, Err(vt_plan::Error::NonUtf8ToolEntry { path }) if path == entry));
    }
}
