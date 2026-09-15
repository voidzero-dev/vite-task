// exec_elf implements the Linux process ABI. Its execution tests need Linux,
// even though Cargo can cross-compile the freestanding artifact on other hosts.
#![cfg(target_os = "linux")]

use std::{
    ffi::OsStr,
    os::unix::ffi::OsStrExt as _,
    process::{Command, Output, Stdio},
};

use ntest::timeout;

const LAUNCHER: &str = env!("CARGO_BIN_FILE_FSPY_ELF_LOADER_fspy_elf_loader_test_launcher");
const FIXTURE: &str = env!("CARGO_BIN_EXE_fixture");

#[test]
#[timeout(10_000)]
fn echo_matches_kernel_exec() {
    assert_dynamic_pie("/bin/echo");
    let mut direct = Command::new("/bin/echo");
    direct.args(["hello", "world"]);
    let mut loaded = launcher();
    loaded.args(["/bin/echo", "custom-argv-zero", "hello", "world"]);
    let direct = output(direct);
    let loaded = output(loaded);
    assert_eq!(loaded.status.code(), Some(0), "{loaded:?}");
    assert_eq!(loaded.stdout, b"hello world\n");
    assert_eq!(loaded.stdout, direct.stdout);
    assert_eq!(loaded.stderr, direct.stderr);
}

#[test]
#[timeout(10_000)]
fn echo_preserves_empty_and_non_utf8_arguments() {
    let mut command = launcher();
    command.args(["/bin/echo", "echo", ""]);
    command.arg(OsStr::from_bytes(b"\xff"));
    let output = output(command);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(output.stdout, b" \xff\n");
    assert!(output.stderr.is_empty());
}

#[test]
#[timeout(10_000)]
fn initializes_runtime_bss_tls_and_auxiliary_vector() {
    assert_dynamic_pie(FIXTURE);
    let mut command = launcher();
    command.args([FIXTURE, "custom-argv-zero", ""]);
    command.arg(OsStr::from_bytes(b"\xff"));
    command.env_clear().env("FIXTURE_IMAGE", FIXTURE).env("EMPTY", "");
    command.env("BYTES", OsStr::from_bytes(b"\xff"));
    let output = output(command);
    assert_eq!(output.status.code(), Some(37), "{output:?}");
    assert_eq!(output.stdout, b"runtime-ok\n");
    assert!(output.stderr.is_empty(), "{output:?}");
}

#[test]
#[timeout(10_000)]
fn rejects_unsupported_static_image() {
    let mut command = launcher();
    command.args([LAUNCHER, "target-argv-zero"]);
    let output = output(command);
    assert_eq!(output.status.code(), Some(101), "{output:?}");
    assert!(str::from_utf8(&output.stderr).unwrap().contains("only ET_DYN"));
}

#[test]
#[timeout(10_000)]
fn rejects_malformed_images() {
    let echo = std::fs::read("/bin/echo").unwrap();
    let mut overflow = echo.clone();
    overflow[32..40].copy_from_slice(&u64::MAX.to_le_bytes());
    let mut wrong_machine = echo.clone();
    wrong_machine[18..20].copy_from_slice(&0_u16.to_le_bytes());
    let parsed = elf::ElfBytes::<elf::endian::LittleEndian>::minimal_parse(&echo).unwrap();
    let load =
        parsed.segments().unwrap().iter().position(|ph| ph.p_type == elf::abi::PT_LOAD).unwrap();
    let header = usize::try_from(parsed.ehdr.e_phoff).unwrap() + load * 56;
    let mut invalid_size = echo.clone();
    invalid_size[header + 32..header + 40].copy_from_slice(&u64::MAX.to_le_bytes());
    let mut invalid_alignment = echo.clone();
    invalid_alignment[header + 48..header + 56].copy_from_slice(&3_u64.to_le_bytes());
    for bytes in [
        b"not ELF".to_vec(),
        echo[..64].to_vec(),
        overflow,
        wrong_machine,
        invalid_size,
        invalid_alignment,
    ] {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), bytes).unwrap();
        let mut command = launcher();
        command.arg(file.path()).arg("invalid-image");
        let output = output(command);
        assert_eq!(output.status.code(), Some(101), "{output:?}");
        assert!(str::from_utf8(&output.stderr).unwrap().contains("panicked at"));
    }
}

#[test]
#[timeout(10_000)]
fn does_not_require_section_headers() {
    let mut bytes = std::fs::read("/bin/echo").unwrap();
    // The ELF loader must not use section metadata, even if it is unusable.
    bytes[40..48].copy_from_slice(&u64::MAX.to_le_bytes());
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), bytes).unwrap();
    let mut command = launcher();
    command.arg(file.path()).args(["echo", "without-sections"]);
    let output = output(command);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(output.stdout, b"without-sections\n");
}

#[test]
#[timeout(10_000)]
fn panic_terminates_with_read_only_stderr() {
    let mut command = launcher();
    // Keep the descriptor valid, but make writes fail with EBADF.
    command.stderr(std::fs::File::open("/dev/null").unwrap());
    // No target argument: the launcher panics while reading its arguments.
    let output = output(command);
    assert_eq!(output.status.code(), Some(101));
    assert!(output.stderr.is_empty());
}

fn launcher() -> Command {
    let mut command = Command::new(LAUNCHER);
    command.stderr(Stdio::piped());
    command
}

fn assert_dynamic_pie(path: &str) {
    let bytes = std::fs::read(path).unwrap();
    let elf = elf::ElfBytes::<elf::endian::LittleEndian>::minimal_parse(&bytes).unwrap();
    assert_eq!(elf.ehdr.e_type, elf::abi::ET_DYN, "fixture must be a dynamic PIE: {path}");
    assert!(elf.segments().unwrap().iter().any(|ph| ph.p_type == elf::abi::PT_INTERP));
}

fn output(mut command: Command) -> Output {
    command.stdin(Stdio::null()).output().expect("cannot run freestanding launcher")
}
