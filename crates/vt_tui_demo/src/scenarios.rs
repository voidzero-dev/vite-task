//! The runs the demo plays: dev servers that keep running, and a CI task
//! made of lint, build and test steps.

use vt_str::Str;
use vt_tui::{NodeId, NodeSpec, TaskTree, Tone};

use crate::sim::{End, Proc, ReplaceClock as _, Reply, Sim, Step};

/// A scripted run: the tree of its tasks, and what they do.
#[derive(Debug)]
pub struct Scenario {
    pub command: &'static str,
    pub tree: TaskTree,
    pub sim: Sim,
}

fn out(text: &str) -> Step {
    Step::Out(Str::from(text))
}

fn status(text: &str) -> Step {
    Step::Status(Some(Str::from(text)), Tone::Normal)
}

fn warn(text: &str) -> Step {
    Step::Status(Some(Str::from(text)), Tone::Warning)
}

fn error(text: &str) -> Step {
    Step::Status(Some(Str::from(text)), Tone::Error)
}

fn leaf(
    tree: &mut TaskTree,
    parent: Option<NodeId>,
    label: &'static str,
    command: &'static str,
) -> NodeId {
    tree.add(parent, NodeSpec::new(label).command(command))
}

// ── `vp run -r dev` ─────────────────────────────────────────────────────────

const VITE_TIME: &str = "\x1b[2m%clock%\x1b[22m \x1b[36m\x1b[1m[vite]\x1b[22m\x1b[39m";

fn vite_banner(port: u16, ready_ms: u16) -> Str {
    vt_str::format!(
        "\n  \x1b[32m\x1b[1mVITE\x1b[22m v8.3.2\x1b[39m  \x1b[2mready in \x1b[0m\x1b[1m{ready_ms}\x1b[22m\x1b[2m ms\x1b[22m\n\n  \x1b[32m➜\x1b[39m  \x1b[1mLocal\x1b[22m:   \x1b[36mhttp://localhost:\x1b[1m{port}\x1b[22m/\x1b[39m\n  \x1b[32m➜\x1b[39m  \x1b[1mNetwork\x1b[22m: \x1b[2muse \x1b[22m\x1b[1m--host\x1b[22m\x1b[2m to expose\x1b[22m\n  \x1b[2m\x1b[32m➜\x1b[39m\x1b[22m\x1b[2m  press \x1b[22m\x1b[1mh + enter\x1b[22m\x1b[2m to show help\x1b[22m\n"
    )
}

fn vite_reply(line: &str, port: u16, clock: &str) -> Reply {
    let time = VITE_TIME.replace_clock(clock);
    let text = match line {
        "h" => Str::from(
            "\n  \x1b[1mShortcuts\x1b[22m\n  \x1b[2mpress \x1b[22m\x1b[1mr + enter\x1b[22m\x1b[2m to restart the server\x1b[22m\n  \x1b[2mpress \x1b[22m\x1b[1mu + enter\x1b[22m\x1b[2m to show server url\x1b[22m\n  \x1b[2mpress \x1b[22m\x1b[1mo + enter\x1b[22m\x1b[2m to open in browser\x1b[22m\n  \x1b[2mpress \x1b[22m\x1b[1mc + enter\x1b[22m\x1b[2m to clear console\x1b[22m\n  \x1b[2mpress \x1b[22m\x1b[1mq + enter\x1b[22m\x1b[2m to quit\x1b[22m\n",
        ),
        "r" => vt_str::format!("{time} \x1b[32mserver restarted.\x1b[39m\n"),
        "u" => vt_str::format!(
            "  \x1b[32m➜\x1b[39m  \x1b[1mLocal\x1b[22m:   \x1b[36mhttp://localhost:\x1b[1m{port}\x1b[22m/\x1b[39m\n  \x1b[32m➜\x1b[39m  \x1b[1mNetwork\x1b[22m: \x1b[2muse \x1b[22m\x1b[1m--host\x1b[22m\x1b[2m to expose\x1b[22m\n"
        ),
        "o" => vt_str::format!(
            "{time} \x1b[2mopening http://localhost:{port}/ in the browser\x1b[22m\n"
        ),
        "q" => {
            return Reply {
                steps: vec![out("\x1b[2mshutting down the server\x1b[22m\n")],
                end: Some(End::Succeed),
            };
        }
        _ => return Reply::default(),
    };
    Reply { steps: vec![Step::Out(text)], end: None }
}

fn hmr(path: &str) -> Str {
    vt_str::format!(
        "{VITE_TIME} \x1b[2m(client)\x1b[22m \x1b[32mhmr update \x1b[39m\x1b[2m{path}\x1b[22m\n"
    )
}

const TSC_CLEAR: &str = "\x1b[2J\x1b[3J\x1b[H";

#[expect(clippy::too_many_lines, reason = "one script per task, kept together to read as a run")]
#[must_use]
pub fn dev() -> Scenario {
    let mut tree = TaskTree::new();
    let ui =
        tree.add(None, NodeSpec::new("@acme/ui#dev").command("tsc --watch").accepts_input(true));
    let api = tree.add(None, NodeSpec::new("@acme/api#dev"));
    let api_types = tree.add(Some(api), NodeSpec::new("wrangler types"));
    let api_dev = tree.add(Some(api), NodeSpec::new("wrangler dev").accepts_input(true));
    let web = tree.add(None, NodeSpec::new("@acme/web#dev").command("vp dev").accepts_input(true));
    let docs =
        tree.add(None, NodeSpec::new("@acme/docs#dev").command("vp dev").accepts_input(true));

    // tsc is a third-party tool: it reports no status and never says it's ready.
    let mut ui_proc = Proc::new(ui, "@acme/ui#dev");
    ui_proc.script = vec![
        (
            0,
            Step::Out(vt_str::format!(
                "{TSC_CLEAR}%clock% - Starting compilation in watch mode...\n\n"
            )),
        ),
        (1400, out("%clock% - Found 0 errors. Watching for file changes.\n\n")),
        (
            9000,
            Step::Out(vt_str::format!(
                "{TSC_CLEAR}%clock% - File change detected. Starting incremental compilation...\n\n"
            )),
        ),
        (
            9700,
            out(
                "src/button.tsx(12,5): error TS2322: Type 'string' is not assignable to type 'number'.\n\n12     size: props.size ?? 'medium',\n       ~~~~\n\n%clock% - Found 1 error. Watching for file changes.\n\n",
            ),
        ),
        (
            17000,
            Step::Out(vt_str::format!(
                "{TSC_CLEAR}%clock% - File change detected. Starting incremental compilation...\n\n"
            )),
        ),
        (17600, out("%clock% - Found 0 errors. Watching for file changes.\n\n")),
    ];

    let mut types_proc = Proc::new(api_types, "wrangler types");
    types_proc.script = vec![
        (0, status("generating")),
        (
            300,
            out(
                "\x1b[2mGenerating project types...\x1b[22m\n\ninterface Env {\n\tDB: D1Database;\n\tASSETS: Fetcher;\n}\n\n",
            ),
        ),
        (700, out("✨ Types written to \x1b[1mworker-configuration.d.ts\x1b[22m\n")),
        (700, status("2 bindings")),
    ];
    types_proc.end = Some((800, End::Succeed));

    let mut wrangler_proc = Proc::new(api_dev, "wrangler dev");
    wrangler_proc.after = vec![1];
    wrangler_proc.script = vec![
        (0, status("starting")),
        (
            200,
            out(
                "\n \x1b[33m⛅️ wrangler 4.42.0\x1b[39m\n\x1b[33m───────────────────\x1b[39m\nYour Worker has access to the following bindings:\nBinding                Resource       Mode\n\x1b[2menv.DB (acme-db)\x1b[22m       D1 Database    local\n\x1b[2menv.ASSETS\x1b[22m             Assets         local\n\n⎔ Starting local server...\n",
            ),
        ),
        (
            1500,
            out("\x1b[2m[wrangler:info]\x1b[22m Ready on \x1b[36mhttp://localhost:8787\x1b[39m\n"),
        ),
        (1500, status("http://localhost:8787")),
        (1500, Step::Ready),
    ];
    for (index, (path, ms)) in
        [("/api/todos", 12), ("/api/todos/3", 8), ("/api/user", 21), ("/api/todos", 9)]
            .iter()
            .enumerate()
    {
        let at = 4000 + u64::try_from(index).unwrap_or(0) * 3700;
        wrangler_proc.script.push((
            at,
            Step::Out(vt_str::format!(
                "\x1b[2m[wrangler:info]\x1b[22m GET {path} \x1b[32m200\x1b[39m OK ({ms}ms)\n"
            )),
        ));
    }

    let mut web_proc = Proc::new(web, "@acme/web#dev");
    web_proc.script = vec![
        (0, status("starting")),
        (450, Step::Out(vite_banner(5173, 412))),
        (450, status("http://localhost:5173")),
        (450, Step::Ready),
    ];
    for (index, path) in [
        "/src/App.tsx",
        "/src/pages/profile.tsx",
        "/src/App.tsx",
        "/src/styles.css",
        "/src/components/nav.tsx",
    ]
    .iter()
    .enumerate()
    {
        let at = 3200 + u64::try_from(index).unwrap_or(0) * 2900;
        web_proc.script.push((at, Step::Out(hmr(path))));
    }
    web_proc.responder = Some(|line, clock| vite_reply(line, 5173, clock));

    let mut docs_proc = Proc::new(docs, "@acme/docs#dev");
    docs_proc.script = vec![
        (0, status("starting")),
        (650, Step::Out(vite_banner(5174, 598))),
        (650, status("http://localhost:5174")),
        (650, Step::Ready),
        (
            7400,
            Step::Out(vt_str::format!(
                "{VITE_TIME} \x1b[2m(client)\x1b[22m \x1b[32mpage reload \x1b[39m\x1b[2mguide/getting-started.md\x1b[22m\n"
            )),
        ),
    ];
    docs_proc.responder = Some(|line, clock| vite_reply(line, 5174, clock));

    let sim = Sim::new(vec![ui_proc, types_proc, wrangler_proc, web_proc, docs_proc], 8);
    Scenario { command: "vp run -r dev", tree, sim }
}

// ── `vp run verify` ─────────────────────────────────────────────────────────

const LINT_OUTPUT: &str = "\n  \x1b[33m⚠\x1b[39m \x1b[33meslint(no-unused-vars)\x1b[39m: Variable 'legacyTheme' is declared but never used.\n   \x1b[2m╭─[\x1b[22m\x1b[36mpackages/ui/src/theme.ts\x1b[39m:14:7\x1b[2m]\x1b[22m\n \x1b[2m13 │\x1b[22m\n \x1b[2m14 │\x1b[22m const legacyTheme = createTheme({ dark: false })\n    \x1b[2m·\x1b[22m       \x1b[33m─────┬─────\x1b[39m\n    \x1b[2m·\x1b[22m            \x1b[33m╰── 'legacyTheme' is declared here\x1b[39m\n \x1b[2m15 │\x1b[22m\n   \x1b[2m╰────\x1b[22m\n  \x1b[36mhelp:\x1b[39m Consider removing this declaration.\n\n  \x1b[33m⚠\x1b[39m \x1b[33mreact-hooks(exhaustive-deps)\x1b[39m: React Hook useEffect has a missing dependency: 'userId'\n   \x1b[2m╭─[\x1b[22m\x1b[36mapps/web/src/pages/profile.tsx\x1b[39m:31:6\x1b[2m]\x1b[22m\n \x1b[2m30 │\x1b[22m     fetchUser(userId).then(setUser)\n \x1b[2m31 │\x1b[22m   }, [])\n    \x1b[2m·\x1b[22m      \x1b[33m─┬\x1b[39m\n    \x1b[2m·\x1b[22m       \x1b[33m╰── add 'userId' to the dependency array\x1b[39m\n   \x1b[2m╰────\x1b[22m\n\nFound \x1b[33m2 warnings\x1b[39m and \x1b[32m0 errors\x1b[39m.\nFinished in 212ms on 412 files with 98 rules using 8 threads.\n";

/// A number in `0..range` that looks random but is the same in every run.
const fn noise(seed: u64, range: u64) -> u64 {
    let mut x = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (x ^ (x >> 31)) % range
}

/// A content hash like the ones bundlers put in file names.
fn content_hash(seed: u64) -> Str {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ_-";
    let mut hash = Str::with_capacity(8);
    for i in 0..8 {
        hash.push(char::from(DIGITS[usize::try_from(noise(seed * 8 + i, 64)).unwrap_or(0)]));
    }
    hash
}

/// A size in kilobytes, from hundredths of one.
fn kilobytes(hundredths: u64) -> Str {
    vt_str::format!("{}.{:02} kB", hundredths / 100, hundredths % 100)
}

type FileSize = (Str, Str, Str);

/// Bundles (and type declarations, with `types`) for each module name.
fn modules(names: &[&str], seed: u64, types: bool) -> Vec<FileSize> {
    let mut files = Vec::new();
    for (index, name) in names.iter().enumerate() {
        let seed = seed * 1000 + index as u64;
        let size = 120 + noise(seed, 2400);
        files.push((vt_str::format!("{name}.mjs"), kilobytes(size), kilobytes(size * 3 / 10)));
        if types {
            let size = 40 + noise(seed + 500, 600);
            files.push((
                vt_str::format!("{name}.d.mts"),
                kilobytes(size),
                kilobytes(size * 4 / 10),
            ));
        }
    }
    files
}

/// Chunks for each route, then the vendor chunks, as `vite build` lists them.
fn chunks(routes: &[&str], vendors: &[&str], seed: u64) -> Vec<FileSize> {
    let mut files = vec![(Str::from("index.html"), kilobytes(46), kilobytes(30))];
    for (index, name) in routes.iter().chain(vendors).enumerate() {
        let seed = seed * 1000 + index as u64;
        let vendor = index >= routes.len();
        let size = if vendor { 1800 + noise(seed, 9000) } else { 160 + noise(seed, 1600) };
        let file = vt_str::format!("assets/{name}-{}.js", content_hash(seed));
        files.push((file, kilobytes(size), kilobytes(size * 32 / 100)));
    }
    files.push((
        vt_str::format!("assets/index-{}.css", content_hash(seed + 7)),
        kilobytes(1248),
        kilobytes(312),
    ));
    files
}

/// Test files with test counts and durations; returns them and the total.
fn test_files(names: &[&str], seed: u64, extension: &str) -> (Vec<(Str, u16, u16)>, u16) {
    let files: Vec<(Str, u16, u16)> = names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let seed = seed * 1000 + index as u64;
            let tests = u16::try_from(3 + noise(seed, 24)).unwrap_or(3);
            let ms = u16::try_from(6 + noise(seed + 77, 180)).unwrap_or(6);
            (vt_str::format!("src/{name}.test.{extension}"), tests, ms)
        })
        .collect();
    let total = files.iter().map(|(_, tests, _)| tests).sum();
    (files, total)
}

fn tsdown(entries: &[FileSize], ms: u16) -> Str {
    let mut text = Str::from(
        "\x1b[34mℹ\x1b[39m tsdown \x1b[2mv0.23.3 powered by rolldown v1.0.0\x1b[22m\n\x1b[34mℹ\x1b[39m entry: \x1b[36msrc/index.ts\x1b[39m\n\x1b[34mℹ\x1b[39m Build start\n",
    );
    for (file, size, gzip) in entries {
        text.push_str(&vt_str::format!(
            "\x1b[34mℹ\x1b[39m \x1b[2mdist/\x1b[22m\x1b[1m{file:<20}\x1b[22m \x1b[2m{size:>9} │ gzip: {gzip}\x1b[22m\n"
        ));
    }
    text.push_str(&vt_str::format!("\x1b[32m✔\x1b[39m Build complete in \x1b[32m{ms}ms\x1b[39m\n"));
    text
}

fn vite_build(modules: u16, files: &[FileSize], seconds: &str, at: [u64; 4]) -> Vec<(u64, Step)> {
    let mut done = Str::default();
    for (file, size, gzip) in files {
        done.push_str(&vt_str::format!(
            "\x1b[2mdist/\x1b[22m\x1b[36m{file:<38}\x1b[39m \x1b[1m\x1b[2m{size:>10}\x1b[22m \x1b[2m│ gzip: {gzip:>8}\x1b[22m\n"
        ));
    }
    done.push_str(&vt_str::format!("\x1b[32m✓ built in {seconds}\x1b[39m\n"));
    vec![
        (
            0,
            out(
                "\x1b[36mvite v8.3.2 \x1b[32mbuilding client environment for production...\x1b[39m\ntransforming...\n",
            ),
        ),
        (at[0], Step::Out(vt_str::format!("\x1b[32m✓\x1b[39m {modules} modules transformed.\n"))),
        (at[1], out("rendering chunks...\n")),
        (at[2], out("computing gzip size...\n")),
        (at[3], Step::Out(done)),
    ]
}

fn vitest_pass(dir: &str, files: &[(Str, u16, u16)], total: u16, duration: &str) -> Str {
    let mut text = vt_str::format!(
        "\n \x1b[7m\x1b[36m RUN \x1b[39m\x1b[27m \x1b[36mv5.0.3 \x1b[39m\x1b[2m/acme/{dir}\x1b[22m\n\n"
    );
    for (file, tests, ms) in files {
        text.push_str(&vt_str::format!(
            " \x1b[32m✓\x1b[39m {file} \x1b[2m({tests} tests)\x1b[22m \x1b[33m{ms}ms\x1b[39m\n"
        ));
    }
    let count = files.len();
    text.push_str(&vt_str::format!(
        "\n\x1b[2m Test Files \x1b[22m \x1b[1m\x1b[32m{count} passed\x1b[39m\x1b[22m\x1b[90m ({count})\x1b[39m\n\x1b[2m      Tests \x1b[22m \x1b[1m\x1b[32m{total} passed\x1b[39m\x1b[22m\x1b[90m ({total})\x1b[39m\n\x1b[2m   Duration \x1b[22m {duration}\n"
    ));
    text
}

const WEB_TEST_FAIL: &str = "\n \x1b[7m\x1b[36m RUN \x1b[39m\x1b[27m \x1b[36mv5.0.3 \x1b[39m\x1b[2m/acme/apps/web\x1b[22m\n\n \x1b[32m✓\x1b[39m src/app.test.tsx \x1b[2m(9 tests)\x1b[22m \x1b[33m88ms\x1b[39m\n \x1b[32m✓\x1b[39m src/components/nav.test.tsx \x1b[2m(7 tests)\x1b[22m \x1b[33m41ms\x1b[39m\n \x1b[31m❯\x1b[39m src/pages/profile.test.tsx \x1b[2m(6 tests | \x1b[22m\x1b[31m1 failed\x1b[39m\x1b[2m)\x1b[22m \x1b[33m212ms\x1b[39m\n   \x1b[31m×\x1b[39m shows the user's name after loading \x1b[33m180ms\x1b[39m\n     \x1b[31m→ expected 'Loading…' to be 'Ada Lovelace' // Object.is equality\x1b[39m\n \x1b[32m✓\x1b[39m src/pages/settings.test.tsx \x1b[2m(20 tests)\x1b[22m \x1b[33m96ms\x1b[39m\n\n\x1b[31m⎯⎯⎯⎯⎯⎯⎯ \x1b[1m\x1b[7m Failed Tests 1 \x1b[27m\x1b[22m ⎯⎯⎯⎯⎯⎯⎯\x1b[39m\n\n\x1b[41m\x1b[1m FAIL \x1b[22m\x1b[49m src/pages/profile.test.tsx\x1b[2m > \x1b[22mProfile\x1b[2m > \x1b[22mshows the user's name after loading\n\x1b[31m\x1b[1mAssertionError\x1b[22m: expected 'Loading…' to be 'Ada Lovelace' // Object.is equality\x1b[39m\n\nExpected: \x1b[32m\"Ada Lovelace\"\x1b[39m\nReceived: \x1b[31m\"Loading…\"\x1b[39m\n\n\x1b[36m \x1b[2m❯\x1b[22m src/pages/profile.test.tsx:\x1b[2m28:41\x1b[22m\x1b[39m\n    \x1b[90m 26|\x1b[39m     render(<Profile userId=\"42\" />)\n    \x1b[90m 27|\x1b[39m     \x1b[35mawait\x1b[39m screen.findByRole(\x1b[32m'heading'\x1b[39m)\n    \x1b[90m 28|\x1b[39m     expect(screen.getByRole(\x1b[32m'heading'\x1b[39m).textContent).toBe(\x1b[32m'Ada Lovelace'\x1b[39m)\n    \x1b[90m   |\x1b[39m                                         \x1b[31m^\x1b[39m\n    \x1b[90m 29|\x1b[39m   })\n\n\x1b[31m\x1b[2m⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯[1/1]⎯\x1b[22m\x1b[39m\n\n\x1b[2m Test Files \x1b[22m \x1b[1m\x1b[31m1 failed\x1b[39m\x1b[22m\x1b[2m | \x1b[22m\x1b[1m\x1b[32m3 passed\x1b[39m\x1b[22m\x1b[90m (4)\x1b[39m\n\x1b[2m      Tests \x1b[22m \x1b[1m\x1b[31m1 failed\x1b[39m\x1b[22m\x1b[2m | \x1b[22m\x1b[1m\x1b[32m41 passed\x1b[39m\x1b[22m\x1b[90m (42)\x1b[39m\n\x1b[2m   Duration \x1b[22m 2.61s\n";

const UI_COMPONENTS: [&str; 24] = [
    "accordion",
    "alert",
    "avatar",
    "badge",
    "breadcrumb",
    "button",
    "card",
    "checkbox",
    "combobox",
    "date-picker",
    "dialog",
    "drawer",
    "dropdown",
    "form",
    "input",
    "menu",
    "pagination",
    "popover",
    "progress",
    "radio",
    "select",
    "slider",
    "switch",
    "tabs",
];
const UTILS_MODULES: [&str; 10] = [
    "index", "format", "dates", "url", "strings", "numbers", "arrays", "objects", "promise",
    "errors",
];
const API_ROUTES: [&str; 6] = ["index", "todos", "users", "auth", "billing", "webhooks"];
const WEB_ROUTES: [&str; 22] = [
    "home",
    "dashboard",
    "profile",
    "settings",
    "billing",
    "team",
    "members",
    "invites",
    "projects",
    "project",
    "issues",
    "issue",
    "search",
    "notifications",
    "onboarding",
    "login",
    "signup",
    "reset-password",
    "integrations",
    "api-keys",
    "audit-log",
    "help",
];
const WEB_VENDORS: [&str; 8] =
    ["react", "react-dom", "router", "query", "zod", "date-fns", "charts", "icons"];
const DOCS_PAGES: [&str; 38] = [
    "guide/index",
    "guide/getting-started",
    "guide/installation",
    "guide/theming",
    "guide/dark-mode",
    "guide/forms",
    "guide/accessibility",
    "guide/testing",
    "guide/deploying",
    "guide/migrating",
    "guide/faq",
    "guide/troubleshooting",
    "api/accordion",
    "api/alert",
    "api/avatar",
    "api/badge",
    "api/button",
    "api/card",
    "api/checkbox",
    "api/combobox",
    "api/date-picker",
    "api/dialog",
    "api/drawer",
    "api/dropdown",
    "api/form",
    "api/input",
    "examples/index",
    "examples/dashboard",
    "examples/settings",
    "examples/billing",
    "examples/auth",
    "examples/data-table",
    "examples/kanban",
    "examples/calendar",
    "examples/chat",
    "examples/landing",
    "examples/pricing",
    "examples/blog",
];
const UTILS_TESTS: [&str; 10] = [
    "format", "dates", "url", "strings", "numbers", "arrays", "objects", "promise", "errors",
    "cache",
];
const API_TESTS: [&str; 8] =
    ["todos", "users", "auth", "billing", "webhooks", "rate-limit", "sessions", "cors"];

/// `vp run verify`, where the root `verify` task is
/// `vp lint && vp run -r build && vp run -r test`.
/// The tasks that succeed print long logs, as they do in a real monorepo.
#[must_use]
#[expect(clippy::too_many_lines, reason = "one script per task, kept together to read as a run")]
pub fn verify(fail: bool) -> Scenario {
    let mut tree = TaskTree::new();
    let root = tree.add(None, NodeSpec::new("acme#verify"));
    let lint = tree.add(Some(root), NodeSpec::new("vp lint"));
    let build = tree.add(Some(root), NodeSpec::new("vp run -r build"));
    let utils_build = leaf(&mut tree, Some(build), "@acme/utils#build", "tsdown");
    let ui_build = tree.add(Some(build), NodeSpec::new("@acme/ui#build"));
    let ui_tsc = tree.add(Some(ui_build), NodeSpec::new("tsc -b"));
    let ui_tsdown = tree.add(Some(ui_build), NodeSpec::new("tsdown"));
    let api_build = leaf(&mut tree, Some(build), "@acme/api#build", "tsdown");
    let web_build = leaf(&mut tree, Some(build), "@acme/web#build", "vp build");
    let docs_build = leaf(&mut tree, Some(build), "@acme/docs#build", "vp build");
    let test = tree.add(Some(root), NodeSpec::new("vp run -r test"));
    let utils_test = leaf(&mut tree, Some(test), "@acme/utils#test", "vitest run");
    let ui_test = leaf(&mut tree, Some(test), "@acme/ui#test", "vitest run");
    let api_test = leaf(&mut tree, Some(test), "@acme/api#test", "vitest run");
    let web_test = leaf(&mut tree, Some(test), "@acme/web#test", "vitest run");

    // Indices into `procs`, in the order they're pushed below.
    let (p_lint, p_utils, p_ui_tsc, p_ui_tsdown, p_api, p_web, p_docs) = (0, 1, 2, 3, 4, 5, 6);
    let builds = vec![p_utils, p_ui_tsc, p_ui_tsdown, p_api, p_web, p_docs];

    let mut procs = Vec::new();

    let mut lint_proc = Proc::new(lint, "vp lint");
    lint_proc.script = vec![
        (0, status("linting")),
        (500, status("180 files")),
        (1000, status("412 files · 2 warnings")),
        (1100, out(LINT_OUTPUT)),
        (1100, warn("412 files · 2 warnings")),
    ];
    lint_proc.end = Some((1200, End::Succeed));
    procs.push(lint_proc);

    let utils_files = modules(&UTILS_MODULES, 1, true);
    let mut utils = Proc::new(utils_build, "@acme/utils#build");
    utils.after = vec![p_lint];
    utils.cache_hit = true;
    utils.saved_ms = 900;
    utils.script = vec![
        (0, Step::Out(tsdown(&utils_files, 182))),
        (0, status(&vt_str::format!("{} files", utils_files.len()))),
    ];
    utils.end = Some((0, End::Succeed));
    procs.push(utils);

    let mut ui_tsc_proc = Proc::new(ui_tsc, "tsc -b");
    ui_tsc_proc.after = vec![p_lint];
    ui_tsc_proc.end = Some((1700, End::Succeed));
    procs.push(ui_tsc_proc);

    let mut ui_files = modules(&UI_COMPONENTS, 2, true);
    ui_files.push((Str::from("style.css"), kilobytes(1130), kilobytes(274)));
    let mut ui_tsdown_proc = Proc::new(ui_tsdown, "@acme/ui#build");
    ui_tsdown_proc.after = vec![p_lint, p_ui_tsc];
    ui_tsdown_proc.script = vec![
        (0, status("bundling")),
        (900, Step::Out(tsdown(&ui_files, 874))),
        (900, status(&vt_str::format!("{} files", ui_files.len()))),
    ];
    ui_tsdown_proc.end = Some((1000, End::Succeed));
    procs.push(ui_tsdown_proc);

    let api_files = modules(&API_ROUTES, 3, false);
    let mut api = Proc::new(api_build, "@acme/api#build");
    api.after = vec![p_lint];
    api.deps = vec![p_utils];
    api.script = vec![
        (0, status("bundling")),
        (700, Step::Out(tsdown(&api_files, 655))),
        (700, status(&vt_str::format!("{} files", api_files.len()))),
    ];
    api.end = Some((800, End::Succeed));
    procs.push(api);

    let web_files = chunks(&WEB_ROUTES, &WEB_VENDORS, 4);
    let mut web = Proc::new(web_build, "@acme/web#build");
    web.after = vec![p_lint];
    web.deps = vec![p_utils, p_ui_tsdown];
    web.script = vec![
        (0, status("transforming · 0 modules")),
        (400, status("transforming · 214 modules")),
        (900, status("transforming · 590 modules")),
        (1500, status("transforming · 812 modules")),
        (1900, status("rendering chunks")),
        (2500, status("computing gzip size")),
        (2900, status(&vt_str::format!("812 modules · {} files", web_files.len()))),
    ];
    web.script.extend(vite_build(812, &web_files, "2.86s", [1500, 1900, 2500, 2900]));
    web.script.sort_by_key(|(at, _)| *at);
    web.end = Some((3000, End::Succeed));
    procs.push(web);

    let mut docs_files: Vec<FileSize> = DOCS_PAGES
        .iter()
        .enumerate()
        .map(|(index, page)| {
            let size = 900 + noise(5000 + index as u64, 3200);
            (vt_str::format!("{page}.html"), kilobytes(size), kilobytes(size * 3 / 10))
        })
        .collect();
    docs_files
        .extend(chunks(&["app", "search", "playground"], &["vue", "shiki"], 6).into_iter().skip(1));
    let mut docs = Proc::new(docs_build, "@acme/docs#build");
    docs.after = vec![p_lint];
    docs.deps = vec![p_ui_tsdown];
    docs.script = vec![
        (0, status("transforming · 0 modules")),
        (500, status("transforming · 260 modules")),
        (1300, status("transforming · 433 modules")),
        (1700, status("rendering 38 pages")),
        (2300, status(&vt_str::format!("38 pages · {} files", docs_files.len()))),
    ];
    docs.script.extend(vite_build(433, &docs_files, "2.29s", [1300, 1700, 2000, 2300]));
    docs.script.sort_by_key(|(at, _)| *at);
    docs.end = Some((2400, End::Succeed));
    procs.push(docs);

    let (utils_tests, utils_total) = test_files(&UTILS_TESTS, 7, "ts");
    let mut utils_test_proc = Proc::new(utils_test, "@acme/utils#test");
    utils_test_proc.after.clone_from(&builds);
    utils_test_proc.cache_hit = true;
    utils_test_proc.saved_ms = 1400;
    utils_test_proc.script = vec![
        (0, Step::Out(vitest_pass("packages/utils", &utils_tests, utils_total, "412ms"))),
        (0, status(&vt_str::format!("{utils_total} passed"))),
    ];
    utils_test_proc.end = Some((0, End::Succeed));
    procs.push(utils_test_proc);

    let (ui_tests, ui_total) = test_files(&UI_COMPONENTS, 8, "tsx");
    let mut ui_test_proc = Proc::new(ui_test, "@acme/ui#test");
    ui_test_proc.after.clone_from(&builds);
    ui_test_proc.script = vec![
        (0, status("6 files · 52 tests")),
        (700, status("14 files · 131 tests")),
        (1300, status("21 files · 197 tests")),
        (1900, Step::Out(vitest_pass("packages/ui", &ui_tests, ui_total, "1.94s"))),
        (1900, status(&vt_str::format!("{ui_total} passed"))),
    ];
    ui_test_proc.end = Some((2000, End::Succeed));
    procs.push(ui_test_proc);

    let (api_tests, api_total) = test_files(&API_TESTS, 9, "ts");
    let mut api_test_proc = Proc::new(api_test, "@acme/api#test");
    api_test_proc.after.clone_from(&builds);
    api_test_proc.script = vec![
        (0, status("3 files · 24 tests")),
        (1700, Step::Out(vitest_pass("apps/api", &api_tests, api_total, "1.71s"))),
        (1700, status(&vt_str::format!("{api_total} passed"))),
    ];
    api_test_proc.end = Some((1800, End::Succeed));
    procs.push(api_test_proc);

    let mut web_test_proc = Proc::new(web_test, "@acme/web#test");
    web_test_proc.after = builds;
    web_test_proc.script =
        vec![(0, status("2 files · 16 tests")), (800, status("3 files · 22 tests"))];
    if fail {
        web_test_proc.script.push((1300, error("1 failed · 21 tests")));
        web_test_proc.script.push((2500, Step::Out(Str::from(WEB_TEST_FAIL))));
        web_test_proc.script.push((2500, error("1 failed · 41 passed")));
        web_test_proc.end = Some((2600, End::Fail));
    } else {
        let (web_tests, web_total) =
            test_files(&["app", "components/nav", "pages/profile", "pages/settings"], 10, "tsx");
        web_test_proc
            .script
            .push((2500, Step::Out(vitest_pass("apps/web", &web_tests, web_total, "2.58s"))));
        web_test_proc.script.push((2500, status(&vt_str::format!("{web_total} passed"))));
        web_test_proc.end = Some((2600, End::Succeed));
    }
    procs.push(web_test_proc);

    let sim = Sim::new(procs, 4);
    Scenario { command: "vp run verify", tree, sim }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use vt_tui::{Options, TaskState, Tui};

    use super::*;

    #[test]
    fn lint_report_keeps_its_last_line() {
        let mut tree = TaskTree::new();
        let lint = tree.add(None, NodeSpec::new("vp lint"));
        let mut tui = Tui::new(tree, Options::new(100, 30, "x"));
        tui.task_output(lint, LINT_OUTPUT.as_bytes());
        tui.set_state(lint, TaskState::Failed);
        tui.tick(Duration::from_secs(1));
        tui.finish(&b""[..]);
        let out = tui.render();
        let text = std::str::from_utf8(&out).unwrap();
        assert!(text.contains("Finished in 212ms"), "{text:?}");
    }
}
