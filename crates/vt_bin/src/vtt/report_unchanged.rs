use std::{
    io::{Read as _, Write as _},
    process::Command,
};

pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if args.first().is_some_and(|arg| arg == "nested") {
        let status = Command::new("vt").args(["run", "--no-cache", "report"]).status()?;
        return if status.success() { Ok(()) } else { Err("nested task failed".into()) };
    }
    if args.first().is_some_and(|arg| arg == "corrupt-archive") {
        for version in std::fs::read_dir("node_modules/.vite/task-cache")? {
            for entry in std::fs::read_dir(version?.path())? {
                let path = entry?.path();
                if path.extension().is_some_and(|ext| ext == "zst") {
                    std::fs::write(path, b"invalid archive")?;
                }
            }
        }
        return Ok(());
    }
    if let [mode, input, output] = args
        && mode == "build"
    {
        let input = std::fs::read_to_string(input)?;
        if input.trim() == "initial" {
            std::fs::write(output, "artifact\n")?;
            println!("built artifact");
            return Ok(());
        }
    }
    if args.iter().any(|arg| arg == "cli") {
        let status = Command::new("vt").args(["run", "--report-unchanged"]).status()?;
        if !status.success() {
            return Err("report CLI failed".into());
        }
    } else if let Some(client) = vt_client::Client::from_envs(std::env::vars_os())? {
        client.report_unchanged()?;
        client.report_unchanged()?;
    }
    if args.first().is_none_or(|arg| arg != "fail") {
        println!("command ran");
    }
    match args.first().map(String::as_str) {
        Some("fail") => Err("failed after report".into()),
        Some("interrupt") => super::exit_on_ctrlc::run(),
        Some("stdin") => super::read_stdin::run(),
        Some("tty") => {
            super::check_tty::run();
            Ok(())
        }
        Some("invalid-ipc") => {
            let name = std::env::var_os("VP_RUN_IPC_NAME").ok_or("missing IPC")?;
            let mut stream = socket_ipc::Client::connect(&name)?;
            stream.write_all(&4u32.to_le_bytes())?;
            stream.write_all(&u32::MAX.to_le_bytes())?;
            stream.flush()?;
            // Wait for rejection so the server cannot miss this connection at shutdown.
            let _ = stream.read(&mut [0u8; 1]);
            Ok(())
        }
        _ => Ok(()),
    }
}
