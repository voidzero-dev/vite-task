use std::borrow::Cow;

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
    // Get the packages/tools directory path
    let tools_dir = std::path::Path::new(&manifest_dir)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("packages")
        .join("tools");
    let tools_dir_str = tools_dir.to_str().unwrap();

    redact_string(
        &mut output,
        &[
            (workspace_root, "<workspace>"),
            (manifest_dir.as_str(), "<manifest_dir>"),
            (tools_dir_str, "<tools>"),
        ],
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
