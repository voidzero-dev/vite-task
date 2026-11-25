// exec_elf implements the Linux process ABI. Its execution tests need Linux,
// even though Cargo can cross-compile the freestanding artifact on other hosts.
#![cfg(target_os = "linux")]

use std::{
    ffi::OsStr,
    mem::{offset_of, size_of},
    os::unix::{ffi::OsStrExt as _, process::CommandExt as _},
    process::{Command, Output, Stdio},
};

use ntest::timeout;

const LAUNCHER: &str = env!("CARGO_BIN_FILE_FSPY_ELF_LOADER_TEST_LAUNCHER");
const FIXTURE_DYN_PIE: &str = env!("FIXTURE_DYN_PIE");
const FIXTURE_DYN_EXEC: &str = env!("FIXTURE_DYN_EXEC");
const FIXTURE_STATIC_EXEC: &str = env!("FIXTURE_STATIC_EXEC");
const FIXTURE_STATIC_PIE: &str = env!("FIXTURE_STATIC_PIE");

#[test]
#[timeout(10_000)]
fn echo_matches_kernel_exec() {
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
fn initializes_dynamic_pie_runtime() {
    check_runtime_image(FIXTURE_DYN_PIE, elf::abi::ET_DYN, true);
}

#[test]
#[timeout(10_000)]
fn initializes_fixed_runtime() {
    check_runtime_image(FIXTURE_DYN_EXEC, elf::abi::ET_EXEC, true);
}

fn configure_runtime_fixture(
    command: &mut Command,
    fixture: &str,
    interpreter: bool,
    fixed_address: bool,
) {
    command.arg("").arg(OsStr::from_bytes(b"\xff"));
    command.env_clear().env("FIXTURE_IMAGE", fixture).env("EMPTY", "");
    command.env("BYTES", OsStr::from_bytes(b"\xff"));
    if interpreter {
        command.env("EXPECT_INTERPRETER", "1");
    }
    if fixed_address {
        command.env("EXPECT_FIXED_ADDRESS", "1");
    }
}

fn run_loaded_fixture(fixture: &str, interpreter: bool, fixed_address: bool) -> Output {
    let mut command = launcher();
    command.args([fixture, "custom-argv-zero"]);
    configure_runtime_fixture(&mut command, fixture, interpreter, fixed_address);
    output(command)
}

fn check_runtime_image(fixture: &str, image_type: u16, interpreter: bool) {
    assert_image(fixture, image_type, interpreter);
    let fixed_address = image_type == elf::abi::ET_EXEC;

    // Run each fixture through kernel execve first so fixture/toolchain issues
    // are distinguished from loader regressions.
    let mut direct = Command::new(fixture);
    direct.arg0("custom-argv-zero").stderr(Stdio::piped());
    configure_runtime_fixture(&mut direct, fixture, interpreter, fixed_address);
    let direct = output(direct);
    assert_eq!(direct.status.code(), Some(37), "kernel exec: {direct:?}");
    assert_eq!(direct.stdout, b"runtime-ok\n");
    assert!(direct.stderr.is_empty(), "kernel exec: {direct:?}");

    let loaded = run_loaded_fixture(fixture, interpreter, fixed_address);
    assert_eq!(loaded.status.code(), Some(37), "loader exec: {loaded:?}");
    assert_eq!(loaded.stdout, direct.stdout);
    assert_eq!(loaded.stderr, direct.stderr);
}

#[test]
#[timeout(10_000)]
fn initializes_static_runtime() {
    check_runtime_image(FIXTURE_STATIC_EXEC, elf::abi::ET_EXEC, false);
}

#[test]
#[timeout(10_000)]
fn initializes_static_pie_runtime() {
    check_runtime_image(FIXTURE_STATIC_PIE, elf::abi::ET_DYN, false);
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
    let mut bytes = std::fs::read(FIXTURE_DYN_EXEC).unwrap();
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
    write_u64(&mut bytes, offset_of!(libc::Elf64_Ehdr, e_entry), entry);
    for (index, ph) in loads {
        let phdr_offset = table + index * size_of::<libc::Elf64_Phdr>();
        write_u64(
            &mut bytes,
            phdr_offset + offset_of!(libc::Elf64_Phdr, p_vaddr),
            ph.p_vaddr - base + launcher_base,
        );
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
    let base = std::fs::read(FIXTURE_DYN_PIE).unwrap();
    let mut overflow = base.clone();
    write_u64(&mut overflow, offset_of!(libc::Elf64_Ehdr, e_phoff), u64::MAX);
    let mut wrong_machine = base.clone();
    write_u16(&mut wrong_machine, offset_of!(libc::Elf64_Ehdr, e_machine), 0);
    let mut unsupported_type = base.clone();
    write_u16(&mut unsupported_type, offset_of!(libc::Elf64_Ehdr, e_type), elf::abi::ET_REL);
    let parsed = elf::ElfBytes::<elf::endian::LittleEndian>::minimal_parse(&base).unwrap();
    let load =
        parsed.segments().unwrap().iter().position(|ph| ph.p_type == elf::abi::PT_LOAD).unwrap();
    let phdr = usize::try_from(parsed.ehdr.e_phoff).unwrap() + load * size_of::<libc::Elf64_Phdr>();
    let mut invalid_size = base.clone();
    write_u64(&mut invalid_size, phdr + offset_of!(libc::Elf64_Phdr, p_filesz), u64::MAX);
    let mut invalid_alignment = base.clone();
    write_u64(&mut invalid_alignment, phdr + offset_of!(libc::Elf64_Phdr, p_align), 3);
    for (case, bytes) in [
        ("truncated header", b"not ELF".to_vec()),
        ("truncated program headers", base[..size_of::<libc::Elf64_Ehdr>()].to_vec()),
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
    let mut bytes = std::fs::read(FIXTURE_DYN_PIE).unwrap();
    // The ELF loader must not use section metadata, even if it is unusable.
    write_u64(&mut bytes, offset_of!(libc::Elf64_Ehdr, e_shoff), u64::MAX);
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), bytes).unwrap();
    let output = run_loaded_fixture(file.path().to_str().unwrap(), true, false);
    assert_eq!(output.status.code(), Some(37), "{output:?}");
    assert_eq!(output.stdout, b"runtime-ok\n");
    assert!(output.stderr.is_empty(), "{output:?}");
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

fn assert_image(path: &str, image_type: u16, interpreter: bool) {
    let bytes = std::fs::read(path).unwrap();
    let elf = elf::ElfBytes::<elf::endian::LittleEndian>::minimal_parse(&bytes).unwrap();
    assert_eq!(elf.ehdr.e_type, image_type, "unexpected fixture image type: {path}");
    let has_interp = elf.segments().unwrap().iter().any(|ph| ph.p_type == elf::abi::PT_INTERP);
    assert_eq!(has_interp, interpreter, "unexpected PT_INTERP: {path}");
}

fn write_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + size_of::<u16>()].copy_from_slice(&value.to_le_bytes());
}

fn write_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + size_of::<u64>()].copy_from_slice(&value.to_le_bytes());
}

fn output(mut command: Command) -> Output {
    command.stdin(Stdio::null()).output().expect("cannot run freestanding launcher")
}
