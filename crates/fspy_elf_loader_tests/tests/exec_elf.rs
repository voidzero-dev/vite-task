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
const FIXTURE_EXEC: &str = env!("CARGO_BIN_EXE_fixture_exec");

#[test]
#[timeout(10_000)]
fn echo_matches_kernel_exec() {
    assert_dynamic_image("/bin/echo", elf::abi::ET_DYN);
    let mut direct = Command::new("/bin/echo");
    direct.args(["hello", "world"]);
    let mut loaded = launcher();
    // BusyBox selects its applet through argv[0]. The runtime fixtures below
    // exercise custom argv[0] independently of the host's echo implementation.
    loaded.args(["/bin/echo", "echo", "hello", "world"]);
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
    check_runtime(FIXTURE, elf::abi::ET_DYN);
}

#[test]
#[timeout(10_000)]
fn initializes_fixed_runtime() {
    check_runtime(FIXTURE_EXEC, elf::abi::ET_EXEC);
}

fn check_runtime(fixture: &str, image_type: u16) {
    assert_dynamic_image(fixture, image_type);
    let mut command = launcher();
    command.args([fixture, "custom-argv-zero", ""]);
    command.arg(OsStr::from_bytes(b"\xff"));
    command.env_clear().env("FIXTURE_IMAGE", fixture).env("EMPTY", "");
    command.env("BYTES", OsStr::from_bytes(b"\xff"));
    if image_type == elf::abi::ET_EXEC {
        command.env("EXPECT_FIXED_ADDRESS", "1");
    }
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
    assert!(str::from_utf8(&output.stderr).unwrap().contains("executable has no PT_INTERP"));
}

#[test]
#[timeout(10_000)]
fn rejects_fixed_image_overlapping_launcher() {
    let launcher_bytes = std::fs::read(LAUNCHER).unwrap();
    let launcher_elf =
        elf::ElfBytes::<elf::endian::LittleEndian>::minimal_parse(&launcher_bytes).unwrap();
    let launcher_base = launcher_elf
        .segments()
        .unwrap()
        .iter()
        .find(|ph| ph.p_type == elf::abi::PT_LOAD)
        .unwrap()
        .p_vaddr;
    let mut bytes = std::fs::read(FIXTURE_EXEC).unwrap();
    let parsed = elf::ElfBytes::<elf::endian::LittleEndian>::minimal_parse(&bytes).unwrap();
    let table = usize::try_from(parsed.ehdr.e_phoff).unwrap();
    let loads: Vec<_> = parsed
        .segments()
        .unwrap()
        .iter()
        .enumerate()
        .filter(|(_, ph)| ph.p_type == elf::abi::PT_LOAD)
        .collect();
    let base = loads[0].1.p_vaddr;
    let entry = parsed.ehdr.e_entry - base + launcher_base;
    bytes[24..32].copy_from_slice(&entry.to_le_bytes());
    for (index, ph) in loads {
        let offset = table + index * 56 + 16;
        bytes[offset..offset + 8]
            .copy_from_slice(&(ph.p_vaddr - base + launcher_base).to_le_bytes());
    }
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), bytes).unwrap();
    let mut command = launcher();
    command.arg(file.path()).arg("overlapping-image");
    let output = output(command);
    assert_eq!(output.status.code(), Some(101), "{output:?}");
    assert!(
        str::from_utf8(&output.stderr).unwrap().contains("cannot reserve fixed ELF address range"),
        "{output:?}"
    );
}

#[test]
#[ignore = "requires Node.js"]
#[timeout(10_000)]
fn node_runs_javascript() {
    // Resolve the actual executable even when PATH contains a Node wrapper.
    let mut resolve = Command::new("node");
    resolve.args(["-p", "process.execPath"]);
    let resolved = output(resolve);
    assert!(resolved.status.success(), "{resolved:?}");
    let node = OsStr::from_bytes(resolved.stdout.strip_suffix(b"\n").unwrap());
    let script =
        "console.log(require('node:crypto').createHash('sha256').update('loader').digest('hex'))";
    let mut direct = Command::new(node);
    direct.args(["-e", script]);
    let mut loaded = launcher();
    loaded.arg(node).args(["node", "-e", script]);
    let direct = output(direct);
    let loaded = output(loaded);
    assert_eq!(direct.status.code(), Some(0), "{direct:?}");
    assert_eq!(loaded.status.code(), Some(0), "{loaded:?}");
    assert_eq!(loaded.stdout, direct.stdout);
    assert_eq!(loaded.stderr, direct.stderr);
}

#[test]
#[timeout(10_000)]
fn rejects_malformed_images() {
    let echo = std::fs::read("/bin/echo").unwrap();
    let mut overflow = echo.clone();
    overflow[32..40].copy_from_slice(&u64::MAX.to_le_bytes());
    let mut wrong_machine = echo.clone();
    wrong_machine[18..20].copy_from_slice(&0_u16.to_le_bytes());
    let mut unsupported_type = echo.clone();
    unsupported_type[16..18].copy_from_slice(&elf::abi::ET_REL.to_le_bytes());
    let parsed = elf::ElfBytes::<elf::endian::LittleEndian>::minimal_parse(&echo).unwrap();
    let load =
        parsed.segments().unwrap().iter().position(|ph| ph.p_type == elf::abi::PT_LOAD).unwrap();
    let header = usize::try_from(parsed.ehdr.e_phoff).unwrap() + load * 56;
    let mut invalid_size = echo.clone();
    invalid_size[header + 32..header + 40].copy_from_slice(&u64::MAX.to_le_bytes());
    let mut invalid_alignment = echo.clone();
    invalid_alignment[header + 48..header + 56].copy_from_slice(&3_u64.to_le_bytes());
    for (case, bytes) in [
        ("truncated header", b"not ELF".to_vec()),
        ("truncated program headers", echo[..64].to_vec()),
        ("overflowing program-header offset", overflow),
        ("wrong architecture", wrong_machine),
        ("unsupported type", unsupported_type),
        ("invalid size", invalid_size),
        ("invalid alignment", invalid_alignment),
    ] {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), bytes).unwrap();
        let mut command = launcher();
        command.arg(file.path()).arg("invalid-image");
        let output = output(command);
        assert_eq!(output.status.code(), Some(101), "{case}: {output:?}");
        assert!(
            str::from_utf8(&output.stderr).unwrap().contains("panicked at"),
            "{case}: {output:?}"
        );
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

fn assert_dynamic_image(path: &str, image_type: u16) {
    let bytes = std::fs::read(path).unwrap();
    let elf = elf::ElfBytes::<elf::endian::LittleEndian>::minimal_parse(&bytes).unwrap();
    assert_eq!(elf.ehdr.e_type, image_type, "unexpected fixture image type: {path}");
    assert!(elf.segments().unwrap().iter().any(|ph| ph.p_type == elf::abi::PT_INTERP));
}

fn output(mut command: Command) -> Output {
    command.stdin(Stdio::null()).output().expect("cannot run freestanding launcher")
}
