#![cfg(target_os = "linux")]

use std::{os::unix::process::ExitStatusExt as _, process::Command};

use anyhow::{Context as _, Result};
use nix::{
    mount::{MsFlags, mount},
    sched::{CloneFlags, unshare},
    unistd::{Gid, Uid},
};

const USAGE: &str = "Usage: vtt small_dev_shm <command> [args...]";

pub fn run(args: &[String]) -> Result<()> {
    let (program, command_args) = parse_command(args)?;
    run_platform(program, command_args)
}

fn parse_command(args: &[String]) -> Result<(&str, &[String])> {
    args.split_first().map(|(program, args)| (program.as_str(), args)).context(USAGE)
}

fn run_platform(program: &str, command_args: &[String]) -> Result<()> {
    let uid = Uid::current().as_raw();
    let gid = Gid::current().as_raw();

    unshare(CloneFlags::CLONE_NEWUSER | CloneFlags::CLONE_NEWNS)
        .context("unshare user and mount namespaces")?;

    std::fs::write("/proc/self/uid_map", format!("0 {uid} 1\n"))
        .context("write /proc/self/uid_map")?;
    match std::fs::write("/proc/self/setgroups", "deny") {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("write /proc/self/setgroups"),
    }
    std::fs::write("/proc/self/gid_map", format!("0 {gid} 1\n"))
        .context("write /proc/self/gid_map")?;

    mount(None::<&str>, "/", None::<&str>, MsFlags::MS_REC | MsFlags::MS_PRIVATE, None::<&str>)
        .context("make / recursively private")?;

    mount(
        Some("tmpfs"),
        "/dev/shm",
        Some("tmpfs"),
        MsFlags::empty(),
        Some("nr_blocks=1,huge=never"),
    )
    .context("mount one-page tmpfs at /dev/shm")?;

    let status = Command::new(program)
        .args(command_args)
        .status()
        .context("run command with constrained /dev/shm")?;
    let code = status.code().unwrap_or_else(|| status.signal().map_or(1, |signal| 128 + signal));
    std::process::exit(code);
}
