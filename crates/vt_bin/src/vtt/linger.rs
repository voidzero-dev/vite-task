use std::{
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
    time::Duration,
};

/// linger `<ms>` `<stdio|ipc>`
///
/// Starts a child that outlives this process by `<ms>` milliseconds, then
/// exits, the way a task can leave a server running behind it:
/// - `stdio`: the child inherits stdout and stderr and keeps them open.
/// - `ipc`: the child connects to the runner's IPC server and keeps the
///   connection open. This process exits once the server has answered the
///   child, so the connection exists before the task ends.
pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let [ms, mode] = args else {
        return Err("Usage: vtt linger <ms> <stdio|ipc>".into());
    };
    let mut command = Command::new(std::env::current_exe()?);
    command.args(["linger-child", ms, mode]).stdin(Stdio::null());
    match mode.as_str() {
        "stdio" => {
            command.spawn()?;
        }
        "ipc" => {
            let mut child = command.stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?;
            let mut ready = String::new();
            BufReader::new(child.stdout.take().unwrap()).read_line(&mut ready)?;
            if ready.trim_end() != "connected" {
                return Err("vtt linger: the child couldn't connect to the runner".into());
            }
        }
        _ => return Err("Usage: vtt linger <ms> <stdio|ipc>".into()),
    }
    println!("left a process running ({mode})");
    Ok(())
}

/// The child started by [`run`].
pub fn run_child(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let [ms, mode] = args else {
        return Err("Usage: vtt linger-child <ms> <stdio|ipc>".into());
    };
    let ms: u64 = ms.parse()?;
    // Held until this process exits.
    let _client = if mode == "ipc" {
        let client = vt_client::Client::from_envs(std::env::vars_os())?
            .ok_or("vtt linger-child: not started by the runner")?;
        // A round trip, so the server has accepted the connection.
        client.get_env("PATH".as_ref(), false)?;
        let mut stdout = std::io::stdout();
        stdout.write_all(b"connected\n")?;
        stdout.flush()?;
        Some(client)
    } else {
        None
    };
    std::thread::sleep(Duration::from_millis(ms));
    Ok(())
}
