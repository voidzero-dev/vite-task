//! Tests for fspy tracing of statically-linked executables (seccomp path).
//! Skipped on musl: the test binary is an artifact dep targeting musl, and when
//! the CI builds with `-crt-static` the binary becomes dynamically linked,
//! defeating the purpose of these tests.
#![cfg(all(target_os = "linux", not(target_env = "musl")))]
use std::path::Path;

use fspy::PathAccessIterable;
use test_log::test;

use crate::test_utils::{assert_contains, assert_not_contains, test_bin_path};

mod test_utils;

async fn track_test_bin(args: &[&str], cwd: Option<&str>) -> PathAccessIterable {
    let mut cmd = fspy::Command::new(test_bin_path());
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    cmd.args(args);
    let tracked_child = cmd.spawn(tokio_util::sync::CancellationToken::new()).await.unwrap();

    let termination = tracked_child.wait_handle.await.unwrap();
    assert!(termination.status.success());

    termination.path_accesses.expect("the tracking region holds every record this run makes")
}

#[test(tokio::test)]
async fn open_read() {
    let accesses = track_test_bin(&["open_read", "/hello"], None).await;
    assert_contains(&accesses, Path::new("/hello"), fspy::PathAccess::Read);
}

#[test(tokio::test)]
async fn open_write() {
    let accesses = track_test_bin(&["open_write", "/hello"], None).await;
    assert_contains(&accesses, Path::new("/hello"), fspy::PathAccess::Write);
}

#[test(tokio::test)]
async fn open_readwrite() {
    let accesses = track_test_bin(&["open_readwrite", "/hello"], None).await;
    assert_contains(&accesses, Path::new("/hello"), fspy::PathAccess::ReadWrite);
}

#[test(tokio::test)]
async fn open_folder_only() {
    // The open of a file fails without reading it, so it isn't recorded.
    let file = tempfile::NamedTempFile::new().unwrap();
    let accesses = track_test_bin(&["open_folder_only", file.path().to_str().unwrap()], None).await;
    assert_not_contains(&accesses, file.path());

    let accesses = track_test_bin(&["open_folder_only", "/hello"], None).await;
    assert_contains(&accesses, Path::new("/hello"), fspy::PathAccess::Read);
}

#[test(tokio::test)]
async fn openat2_read() {
    let accesses = track_test_bin(&["openat2_read", "/hello"], None).await;
    assert_contains(&accesses, Path::new("/hello"), fspy::PathAccess::Read);
}

#[test(tokio::test)]
async fn openat2_write() {
    let accesses = track_test_bin(&["openat2_write", "/hello"], None).await;
    assert_contains(&accesses, Path::new("/hello"), fspy::PathAccess::Write);
}

#[test(tokio::test)]
async fn openat2_readwrite() {
    let accesses = track_test_bin(&["openat2_readwrite", "/hello"], None).await;
    assert_contains(&accesses, Path::new("/hello"), fspy::PathAccess::ReadWrite);
}

#[test(tokio::test)]
async fn mkdir() {
    // The mkdir fails, since `/hello` doesn't exist; the folder is recorded
    // anyway.
    let accesses = track_test_bin(&["mkdir", "/hello/world"], None).await;
    assert_contains(&accesses, Path::new("/hello/world"), fspy::PathAccess::Mkdir);
}

#[test(tokio::test)]
async fn rename_destination() {
    // The rename fails, since `/hello` doesn't exist; the destination is
    // recorded anyway.
    let accesses = track_test_bin(&["rename", "/hello", "/world"], None).await;
    assert_contains(&accesses, Path::new("/world"), fspy::PathAccess::RenameTarget);
}

#[test(tokio::test)]
async fn renameat2_destination() {
    // Relative to a descriptor for `/`. The rename fails, since `/source`
    // doesn't exist; the destination is recorded anyway.
    let accesses = track_test_bin(&["renameat2", "/", "world"], None).await;
    assert_contains(&accesses, Path::new("/world"), fspy::PathAccess::RenameTarget);
}

#[test(tokio::test)]
async fn hard_link_destination() {
    let accesses = track_test_bin(&["hard_link", "/hello", "/world"], None).await;
    assert_contains(&accesses, Path::new("/world"), fspy::PathAccess::Write);
}

#[test(tokio::test)]
async fn symlink_destination() {
    // The symlink fails, since `/hello` doesn't exist; the link is recorded
    // anyway.
    let accesses = track_test_bin(&["symlink", "target", "/hello/world"], None).await;
    assert_contains(&accesses, Path::new("/hello/world"), fspy::PathAccess::Write);
}

#[test(tokio::test)]
async fn open_relative() {
    let accesses = track_test_bin(&["open_read", "hello"], Some("/home")).await;
    assert_contains(&accesses, Path::new("/home/hello"), fspy::PathAccess::Read);
}

#[test(tokio::test)]
async fn readdir() {
    let accesses = track_test_bin(&["readdir", "/home"], None).await;
    assert_contains(&accesses, Path::new("/home"), fspy::PathAccess::ReadDir);
}

#[test(tokio::test)]
async fn stat() {
    let accesses = track_test_bin(&["stat", "/hello"], None).await;
    assert_contains(&accesses, Path::new("/hello"), fspy::PathAccess::Read);
}

#[test(tokio::test)]
async fn execve() {
    let accesses = track_test_bin(&["execve", "/hello"], None).await;
    assert_contains(&accesses, Path::new("/hello"), fspy::PathAccess::Read);
}
