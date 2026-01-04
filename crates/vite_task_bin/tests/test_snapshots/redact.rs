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

fn redact_paths(value: &mut serde_json::Value, redactions: &[(&str, &str)]) {
    use cow_utils::CowUtils as _;
    visit_json(value, &mut |v| {
        if let serde_json::Value::String(s) = v {
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
    });
}

pub fn redact_snapshot(value: &impl Serialize, workspace_root: &str) -> serde_json::Value {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let mut json_value = serde_json::to_value(value).unwrap();
    redact_paths(
        &mut json_value,
        &[(workspace_root, "<workspace>"), (manifest_dir.as_str(), "<manifest_dir>")],
    );

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
