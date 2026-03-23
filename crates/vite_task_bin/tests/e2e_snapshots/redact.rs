use std::borrow::Cow;

#[expect(
    clippy::disallowed_types,
    reason = "String mutation required by regex replace and cow_replace APIs"
)]
fn redact_string(s: &mut String, redactions: &[(&str, &str)]) {
    use cow_utils::CowUtils as _;
    for (from, to) in redactions {
        if let Cow::Owned(mut replaced) = s.as_str().cow_replace(from, to) {
            if cfg!(windows) {
                // Normalize backslashes to forward slashes on Windows
                replaced = replaced.cow_replace("\\", "/").into_owned();
                // Collapse double slashes that arise when an escaped path separator (\\)
                // is only partially replaced (e.g., Debug-format paths end with \\")
                while replaced.contains("//") {
                    replaced = replaced.cow_replace("//", "/").into_owned();
                }
            }
            *s = replaced;
        }
    }
}

#[expect(
    clippy::disallowed_types,
    reason = "String required by regex replace_all and cow_replace APIs; Path required for CARGO_MANIFEST_DIR path manipulation"
)]
pub fn redact_e2e_output(mut output: String, workspace_root: &str) -> String {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();

    // On Windows, canonicalize() may produce verbatim paths (\\?\C:\...) while
    // child processes report paths without the prefix. Try both variants.
    let workspace_root_stripped = workspace_root.strip_prefix(r"\\?\").unwrap_or(workspace_root);

    // On Windows, paths displayed via Debug format ({:?}) have backslashes escaped
    // to double-backslashes. Create escaped variants to match Debug-format output.
    // The full escaped variant (with \\?\ prefix) must be tried first since it's
    // the longest match and prevents leaving a stray "\\?\" in the output.
    let workspace_root_full_escaped = {
        use cow_utils::CowUtils as _;
        workspace_root.cow_replace('\\', r"\\").into_owned()
    };
    let workspace_root_stripped_escaped = {
        use cow_utils::CowUtils as _;
        workspace_root_stripped.cow_replace('\\', r"\\").into_owned()
    };

    let mut redactions: Vec<(&str, &str)> = vec![
        (workspace_root, "<workspace>"),
        (workspace_root_stripped, "<workspace>"),
        (manifest_dir.as_str(), "<manifest_dir>"),
    ];

    // Add escaped variants (longest first for correct matching)
    if workspace_root_full_escaped != workspace_root {
        redactions.insert(0, (&workspace_root_full_escaped, "<workspace>"));
    }
    if workspace_root_stripped_escaped != workspace_root_stripped
        && workspace_root_stripped_escaped != workspace_root_full_escaped
    {
        redactions.insert(1, (&workspace_root_stripped_escaped, "<workspace>"));
    }

    redact_string(&mut output, &redactions);

    // Redact durations like "0ns", "123ms" or "1.23s" to "<duration>"
    let duration_regex = regex::Regex::new(r"\d+(\.\d+)?(ns|ms|s)").unwrap();
    output = duration_regex.replace_all(&output, "<duration>").into_owned();

    // Normalize the ", <duration> saved" suffix in cache hit summaries.
    // When tools are fast (e.g., Rust binaries), saved time may be 0ns and the
    // runner omits the suffix entirely. Stripping it ensures stable snapshots.
    let saved_regex = regex::Regex::new(r",? <duration> saved").unwrap();
    output = saved_regex.replace_all(&output, "").into_owned();

    // Strip "in total" from verbose performance summary (includes time details
    // that may be omitted when saved time is 0).
    {
        use cow_utils::CowUtils as _;
        if let Cow::Owned(replaced) = output.as_str().cow_replace(" in total", "") {
            output = replaced;
        }
    }

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

    // Remove nondeterministic mise warnings from shell startup in cross-platform runners.
    let mise_warning_regex = regex::Regex::new(r"(?m)^mise WARN\s+.*\n?").unwrap();
    output = mise_warning_regex.replace_all(&output, "").into_owned();

    // Sort consecutive diagnostic blocks to handle non-deterministic tool output
    // (e.g., oxlint reports warnings in arbitrary order due to multi-threading).
    // Each block starts with "  ! " and ends at the next empty line.
    output = sort_diagnostic_blocks(&output);

    output
}

#[expect(
    clippy::disallowed_types,
    reason = "String return required because join produces a String"
)]
fn sort_diagnostic_blocks(output: &str) -> String {
    let parts: Vec<&str> = output.split('\n').collect();
    let mut result: Vec<&str> = Vec::new();
    let mut i = 0;

    while i < parts.len() {
        if parts[i].starts_with("  ! ") {
            let mut blocks: Vec<Vec<&str>> = Vec::new();

            loop {
                if i >= parts.len() || !parts[i].starts_with("  ! ") {
                    break;
                }
                let mut block: Vec<&str> = Vec::new();
                while i < parts.len() && !parts[i].is_empty() {
                    block.push(parts[i]);
                    i += 1;
                }
                blocks.push(block);
                // Skip the empty line separator between blocks
                if i < parts.len() && parts[i].is_empty() {
                    i += 1;
                }
            }

            blocks.sort();

            for (j, block) in blocks.iter().enumerate() {
                result.extend_from_slice(block);
                // Restore empty line separators (between blocks + trailing)
                if j < blocks.len() - 1 || i <= parts.len() {
                    result.push("");
                }
            }
        } else {
            result.push(parts[i]);
            i += 1;
        }
    }

    result.join("\n")
}
