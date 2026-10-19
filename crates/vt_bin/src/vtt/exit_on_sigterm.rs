/// exit-on-sigterm
///
/// Sets up a SIGTERM handler, emits a "ready" milestone, then waits.
/// When SIGTERM is received, prints "sigterm received" and exits.
#[cfg(unix)]
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    use signal_hook::{consts::SIGTERM, iterator::Signals};

    let mut signals = Signals::new([SIGTERM])?;

    vt_pty_terminal_test_client::mark_milestone("ready");

    for signal in signals.forever() {
        if signal == SIGTERM {
            break;
        }
    }
    println!("sigterm received");
    Ok(())
}

#[cfg(not(unix))]
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    Err("vtt exit-on-sigterm is only supported on Unix".into())
}
