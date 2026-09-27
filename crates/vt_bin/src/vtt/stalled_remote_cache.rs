use std::io::Read as _;

/// stalled-remote-cache `<command>` \[`<args>`...\]
///
/// Runs `<command>` with `VP_REMOTE_CACHE_URL` set to a loopback endpoint
/// that accepts requests but never responds, then exits with the command's
/// exit code. Emits a "request" milestone when a request arrives. Ctrl-C is
/// left to the command.
pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let [program, args @ ..] = args else {
        return Err("Usage: vtt stalled-remote-cache <command> [args...]".into());
    };
    ctrlc::set_handler(|| {})?;

    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let endpoint = std::format!("http://{}/projects/test", listener.local_addr()?);
    std::thread::spawn(move || {
        for mut stream in listener.incoming().filter_map(Result::ok) {
            std::thread::spawn(move || {
                let mut buf = [0; 4096];
                if stream.read(&mut buf).is_ok_and(|n| n > 0) {
                    pty_terminal_test_client::mark_milestone("request");
                }
                // Hold the connection without responding until the client
                // closes it.
                while stream.read(&mut buf).is_ok_and(|n| n > 0) {}
            });
        }
    });

    let status = std::process::Command::new(program)
        .args(args)
        .env("VP_REMOTE_CACHE_URL", endpoint)
        .status()?;
    std::process::exit(status.code().unwrap_or(1));
}
