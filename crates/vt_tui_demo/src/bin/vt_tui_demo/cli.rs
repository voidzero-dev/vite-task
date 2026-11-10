//! Arguments, agent detection, and the plain output used when stdout isn't a
//! terminal.

use std::{
    io::{IsTerminal as _, Write},
    process::ExitCode,
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use vt_str::Str;
use vt_tui_demo::{Plain, RunNames};

use crate::terminal;

const USAGE: &str = "\
Plays a scripted `vp run` in this terminal.

Usage: vt_tui_demo verify [--pass] [--agent | --no-agent]
       vt_tui_demo dev [--agent | --no-agent]

Scenarios:
  verify      `vp run verify`: `vp lint`, then `vp run -r build` and `vp run -r test`.
              A test fails unless --pass is given.
  dev         `vp run -r dev`: dev servers that keep running until Ctrl-C.

Options:
  --pass      Let every test pass.
  --agent     Draw for an AI agent: no spinner, and a tool's status only once its
              task is ready or finished. Detected from the environment by default.
  --no-agent  Draw for a person, even if an agent is detected.
  -h, --help  Print this help.

The UI starts when stdin and stdout are terminals; otherwise each line a task
writes is printed with its label. Exits with 1 if a task failed, 130 on Ctrl-C.
";

/// Variables an AI agent sets in the environment of the commands it runs: the
/// table of the `std-env` npm package. `AI_AGENT` is for any agent.
const AGENT_VARS: [&str; 10] = [
    "AI_AGENT",
    "CLAUDECODE",
    "CLAUDE_CODE",
    "CODEX_SANDBOX",
    "CODEX_THREAD_ID",
    "CURSOR_AGENT",
    "GEMINI_CLI",
    "COPILOT_AGENT",
    "COPILOT_CLI",
    "OPENCODE",
];

/// How long the run waits for input before it advances on its own.
pub const TICK: Duration = Duration::from_millis(16);

/// What the main loop is woken up with.
pub enum Message {
    /// Bytes read from the terminal.
    #[cfg(not(windows))]
    Input(Vec<u8>),
    /// An event read from the console, whose input isn't a byte stream.
    #[cfg(windows)]
    Event(vt_tui::InputEvent),
    /// A signal to stop, such as SIGINT or SIGTERM.
    Stop,
}

struct Args {
    dev: bool,
    pass: bool,
    agent: Option<bool>,
}

pub fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(Some(args)) => args,
        Ok(None) => {
            let _ = std::io::stdout().write_all(USAGE.as_bytes());
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            let _ = write!(std::io::stderr(), "error: {message}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let scenario = if args.dev { vt_tui_demo::dev() } else { vt_tui_demo::verify(!args.pass) };
    let name = RunNames::new(seed()).next_name();

    let (sender, receiver) = mpsc::channel();
    let stop = sender.clone();
    // If the handler can't be set, the signals keep their default action.
    let _ = ctrlc::set_handler(move || {
        let _ = stop.send(Message::Stop);
    });

    let code = if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        let agent = args.agent.unwrap_or_else(|| detect_agent().is_some());
        terminal::run(scenario, name, agent, sender, &receiver)
    } else {
        plain(Plain::new(scenario, name), &receiver)
    };
    ExitCode::from(code)
}

fn parse_args() -> Result<Option<Args>, Str> {
    let mut scenario = None;
    let mut pass = false;
    let mut agent = None;
    for arg in std::env::args_os().skip(1) {
        match arg.to_str() {
            Some("-h" | "--help") => return Ok(None),
            Some(name @ ("verify" | "dev")) if scenario.is_none() => scenario = Some(name == "dev"),
            Some("--pass") => pass = true,
            Some("--agent") => agent = Some(true),
            Some("--no-agent") => agent = Some(false),
            _ => {
                return Err(vt_str::format!("unexpected argument `{}`", arg.to_string_lossy()));
            }
        }
    }
    let Some(dev) = scenario else { return Err(Str::from("name a scenario: `verify` or `dev`")) };
    if dev && pass {
        return Err(Str::from("`--pass` is for the `verify` scenario"));
    }
    Ok(Some(Args { dev, pass, agent }))
}

/// The first variable of [`AGENT_VARS`] that's set and not empty.
fn detect_agent() -> Option<&'static str> {
    AGENT_VARS
        .into_iter()
        .find(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
}

fn seed() -> u32 {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |time| time.subsec_nanos());
    nanos ^ std::process::id()
}

/// Prints the run as `vp run` does when stdout isn't a terminal. Colors are
/// stripped unless the output takes them, such as with `FORCE_COLOR`.
fn plain(mut run: Plain, receiver: &Receiver<Message>) -> u8 {
    let stdout = std::io::stdout().lock();
    let mut out: Box<dyn Write> = if supports_color::on(supports_color::Stream::Stdout).is_some() {
        Box::new(stdout)
    } else {
        Box::new(anstream::StripStream::new(stdout))
    };
    let start = Instant::now();
    loop {
        let bytes = run.frame(start.elapsed());
        if out.write_all(&bytes).and_then(|()| out.flush()).is_err() {
            return 1;
        }
        if run.is_done() {
            return run.exit_code();
        }
        if matches!(receiver.recv_timeout(TICK), Ok(Message::Stop)) {
            run.cancel(start.elapsed());
        }
    }
}
