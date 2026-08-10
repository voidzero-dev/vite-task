mod access;
mod dirent;
mod exit;
mod open;
mod spawn;
mod stat;

#[cfg(target_os = "linux")]
mod linux_syscall;
