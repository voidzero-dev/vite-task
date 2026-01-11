use std::borrow::Cow;

use serde::Serialize;
use vite_task_graph::config::DEFAULT_PASSTHROUGH_ENVS;

fn visit_json(value: &mut serde_json::Value, f: &mut impl FnMut(&mut serde_json::Value)) {
    f(value);
    match value {
        serde_json::Value::Array(arr) => {
            for item in arr {
                visit_json(item, f);
            }
        }
        serde_json::Value::Object(map) => {
            for (_key, val) in map {
                visit_json(val, f);
            }
        }
        _ => {}
    }
}

fn redact_string_in_json(value: &mut serde_json::Value, redactions: &[(&str, &str)]) {
    visit_json(value, &mut |v| {
        if let serde_json::Value::String(s) = v {
            redact_string(s, redactions);
        }
    });
}

/// Strip Windows executable extensions (case-insensitive) for cross-platform consistency
fn strip_windows_executable_extension(s: &mut String) {
    let lower = s.to_lowercase();
    for ext in [".cmd", ".bat", ".exe", ".com"] {
        if lower.ends_with(ext) {
            s.truncate(s.len() - ext.len());
            break;
        }
    }
}

fn redact_string(s: &mut String, redactions: &[(&str, &str)]) {
    use cow_utils::CowUtils as _;
    for (from, to) in redactions {
        if let Cow::Owned(mut replaced) = s.as_str().cow_replace(from, to) {
            if cfg!(windows) {
                // Also replace with backslashes on Windows
                replaced = replaced.cow_replace("\\", "/").into_owned();
            }
            *s = replaced;
        }
    }
}

pub fn redact_e2e_output(mut output: String, workspace_root: &str) -> String {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();

    redact_string(
        &mut output,
        &[(workspace_root, "<workspace>"), (manifest_dir.as_str(), "<manifest_dir>")],
    );

    // Redact durations like "123ms" or "1.23s" to "<duration>ms" or "<duration>s"
    let duration_regex = regex::Regex::new(r"\d+(\.\d+)?(ms|s)").unwrap();
    output = duration_regex.replace_all(&output, "<duration>").into_owned();

    // Redact thread counts like "using 10 threads" to "using <n> threads"
    let thread_regex = regex::Regex::new(r"using \d+ threads").unwrap();
    output = thread_regex.replace_all(&output, "using <n> threads").into_owned();

    // Remove Node.js experimental warnings (e.g., Type Stripping warnings)
    let node_warning_regex =
        regex::Regex::new(r"(?m)^\(node:\d+\) ExperimentalWarning:.*\n?").unwrap();
    output = node_warning_regex.replace_all(&output, "").into_owned();
    let node_trace_warning_regex = regex::Regex::new(
        r"(?m)^\(Use `node --trace-warnings \.\.\.` to show where the warning was created\)\n?",
    )
    .unwrap();
    output = node_trace_warning_regex.replace_all(&output, "").into_owned();

    output
}

pub fn redact_snapshot(value: &impl Serialize, workspace_root: &str) -> serde_json::Value {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let mut json_value = serde_json::to_value(value).unwrap();

    // On Windows, paths might use either backslashes or forward slashes
    // Try both variants for workspace_root and manifest_dir
    let workspace_root_forward = workspace_root.replace('\\', "/");
    let manifest_dir_forward = manifest_dir.replace('\\', "/");

    redact_string_in_json(
        &mut json_value,
        &[
            (workspace_root, "<workspace>"),
            (workspace_root_forward.as_str(), "<workspace>"),
            (manifest_dir.as_str(), "<manifest_dir>"),
            (manifest_dir_forward.as_str(), "<manifest_dir>"),
        ],
    );

    // Normalize PATH separators for cross-platform consistency (Windows uses ; Unix uses :)
    visit_json(&mut json_value, &mut |v| {
        let serde_json::Value::Object(map) = v else {
            return;
        };
        if let Some(serde_json::Value::String(path)) = map.get_mut("PATH") {
            *path = path.replace(';', ":");
        }
    });

    // Normalize Windows program names and paths by stripping common extensions for cross-platform consistency
    visit_json(&mut json_value, &mut |v| {
        let serde_json::Value::Object(map) = v else {
            return;
        };
        // Normalize program_name field
        if let Some(serde_json::Value::String(program_name)) = map.get_mut("program_name") {
            strip_windows_executable_extension(program_name);
        }
        // Normalize program_path field
        if let Some(serde_json::Value::String(program_path)) = map.get_mut("program_path") {
            strip_windows_executable_extension(program_path);
        }
    });

    visit_json(&mut json_value, &mut |v| {
        let serde_json::Value::Array(array) = v else {
            return;
        };
        let contains_all_default_pass_through_envs =
            DEFAULT_PASSTHROUGH_ENVS.iter().all(|default_pass_through_envs| {
                array.iter().any(|item| {
                    if let serde_json::Value::String(s) = item {
                        s == *default_pass_through_envs
                    } else {
                        false
                    }
                })
            });
        // Remove default pass-through envs from snapshots to reduce noise
        if contains_all_default_pass_through_envs {
            array.retain(|item| {
                if let serde_json::Value::String(s) = item {
                    !DEFAULT_PASSTHROUGH_ENVS.contains(&s.as_str())
                } else {
                    true
                }
            });
            array.push(serde_json::Value::String("<default pass-through envs>".to_string()));
        }
    });

    json_value
}
