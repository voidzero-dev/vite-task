use allocator_api2::{alloc::Allocator, vec::Vec};
use bstr::ByteSlice as _;
use fspy_nostd::{Error, Result};

use super::{Exec, to_vec_in};

/// Maximum shebang recursion depth on Linux (`exec_binprm` allows 5 nested
/// `#!` hops and returns `ELOOP` on the 6th).
#[cfg_attr(
    not(any(target_os = "linux", target_os = "none")),
    expect(dead_code, reason = "used on Linux")
)]
pub const LINUX_MAX_RECURSION: usize = 5;

/// Maximum shebang recursion depth on macOS (XNU allows 1 `#!` hop and
/// rejects a nested `#!` interpreter with `ENOEXEC`).
#[cfg_attr(not(target_os = "macos"), expect(dead_code, reason = "used on macOS"))]
pub const MACOS_MAX_RECURSION: usize = 1;

/// Size of the buffer the macOS kernel reads the shebang line into (`IMG_SHSIZE`).
#[cfg_attr(
    not(any(target_os = "macos", test)),
    expect(dead_code, reason = "used on macOS and in cross-platform unit tests")
)]
pub const MACOS_MAX_LINE_LEN: usize = 512;

/// Size of the buffer the Linux kernel reads the shebang line into (`BINPRM_BUF_SIZE`).
#[cfg_attr(
    not(any(target_os = "linux", target_os = "none", test)),
    expect(dead_code, reason = "used on Linux and in cross-platform unit tests")
)]
pub const LINUX_MAX_LINE_LEN: usize = 256;

#[derive(Debug, Clone)]
pub struct Shebang<A: Allocator> {
    pub interpreter: Vec<u8, A>,
    pub arguments: Vec<Vec<u8, A>, A>,
}

const fn is_whitespace(c: u8) -> bool {
    c == b' ' || c == b'\t'
}

const fn trim_sp_tab_start(mut bytes: &[u8]) -> &[u8] {
    while let Some((&first, rest)) = bytes.split_first() {
        if is_whitespace(first) {
            bytes = rest;
        } else {
            break;
        }
    }
    bytes
}

const fn trim_sp_tab(mut bytes: &[u8]) -> &[u8] {
    bytes = trim_sp_tab_start(bytes);
    while let Some((&last, rest)) = bytes.split_last() {
        if is_whitespace(last) {
            bytes = rest;
        } else {
            break;
        }
    }
    bytes
}

#[derive(Clone, Copy, Debug)]
pub struct ParseShebangOptions {
    pub split_arguments: bool,
    /// How many bytes of the file the kernel reads to find the shebang line.
    pub max_line_len: usize,
    /// Whether the line must end within `max_line_len` bytes.
    pub require_line_end: bool,
    /// Whether `#` ends the line like a newline does (XNU `IS_EOL`).
    pub hash_ends_line: bool,
}

impl ParseShebangOptions {
    #[cfg_attr(
        not(any(target_os = "macos", test)),
        expect(dead_code, reason = "used on macOS and in cross-platform unit tests")
    )]
    #[must_use]
    pub const fn macos() -> Self {
        Self {
            split_arguments: true,
            max_line_len: MACOS_MAX_LINE_LEN,
            require_line_end: true,
            hash_ends_line: true,
        }
    }

    #[cfg_attr(
        not(any(target_os = "linux", target_os = "none", test)),
        expect(dead_code, reason = "used on Linux and in cross-platform unit tests")
    )]
    #[must_use]
    pub const fn linux() -> Self {
        Self {
            split_arguments: false,
            max_line_len: LINUX_MAX_LINE_LEN,
            require_line_end: false,
            hash_ends_line: false,
        }
    }
}

/// Parses the shebang line from an already-peeked file prefix (`buf.len() <=
/// options.max_line_len`), copying the interpreter and arguments into
/// `allocator`.
///
/// Returns [`Error::NOEXEC`] where the kernel would refuse the script instead
/// of running a truncated interpreter path.
pub fn parse_shebang_bytes<A: Allocator + Clone>(
    buf: &[u8],
    options: ParseShebangOptions,
    allocator: A,
) -> Result<Option<Shebang<A>>> {
    let Some(line) = buf.strip_prefix(b"#!") else {
        return Ok(None);
    };

    let line_ends: &[u8] = if options.hash_ends_line { b"\n#\0" } else { b"\n\0" };
    let line = if let Some(line_end) = line.find_byteset(line_ends) {
        &line[..line_end]
    } else if options.require_line_end {
        return Err(Error::NOEXEC);
    } else if buf.len() < options.max_line_len {
        // The whole file fits in the buffer, so nothing was cut off.
        line
    } else {
        // https://github.com/torvalds/linux/blob/v6.12/fs/binfmt_script.c
        // The interpreter path must be followed by a space or tab, or it may be
        // cut off. Like the kernel, look for one in the whole buffer but keep
        // its last byte as a terminator.
        if !line.iter().skip_while(|ch| is_whitespace(**ch)).any(|ch| is_whitespace(*ch)) {
            return Err(Error::NOEXEC);
        }
        &line[..line.len() - 1]
    };

    let line = trim_sp_tab(line);
    let interpreter = line.split(|ch| is_whitespace(*ch)).next().unwrap_or_default();
    if interpreter.is_empty() {
        return Err(Error::NOEXEC);
    }
    let arguments_buf = trim_sp_tab_start(&line[interpreter.len()..]).as_bstr();

    let mut arguments = Vec::new_in(allocator.clone());
    if options.split_arguments {
        arguments.extend(arguments_buf.split(|ch| is_whitespace(*ch)).filter_map(|arg| {
            let arg = trim_sp_tab(arg);
            if arg.is_empty() { None } else { Some(to_vec_in(arg, allocator.clone())) }
        }));
    } else if !arguments_buf.is_empty() {
        arguments.push(to_vec_in(arguments_buf, allocator.clone()));
    }

    Ok(Some(Shebang { interpreter: to_vec_in(interpreter, allocator), arguments }))
}

/// Rewrites `exec` for a parsed `#!` hop, inserting `shebang.interpreter`,
/// `shebang.arguments`, and `script_arg` at the front of `exec.args`.
///
/// Safely handles an empty `exec.args` without panicking.
pub fn apply_shebang<A: Allocator + Clone>(
    exec: &mut Exec<A>,
    shebang: Shebang<A>,
    script_arg: Vec<u8, A>,
) {
    let allocator = exec.allocator().clone();
    let tail_len = exec.args.len().saturating_sub(1);
    let mut new_args = Vec::with_capacity_in(2 + shebang.arguments.len() + tail_len, allocator);
    new_args.push(shebang.interpreter.clone());
    new_args.extend(shebang.arguments);
    new_args.push(script_arg);
    let old_args = core::mem::replace(&mut exec.args, new_args);
    exec.args.extend(old_args.into_iter().skip(1));
    exec.program = shebang.interpreter;
}

#[cfg(test)]
mod tests {
    use allocator_api2::alloc::Global;

    use super::*;

    fn parse(content: &[u8], options: ParseShebangOptions) -> Result<Option<Shebang<Global>>> {
        let len = content.len().min(options.max_line_len);
        parse_shebang_bytes(&content[..len], options, Global)
    }

    fn script_with_interpreter_len(len: usize) -> (std::vec::Vec<u8>, std::vec::Vec<u8>) {
        let interpreter = [b"/".as_slice(), &vec![b'x'; len - 1]].concat();
        let script = [b"#!".as_slice(), &interpreter, b"\necho hi\n"].concat();
        (script, interpreter)
    }

    #[test]
    fn keeps_interpreter_paths_longer_than_128_bytes() {
        for options in [ParseShebangOptions::macos(), ParseShebangOptions::linux()] {
            let (script, interpreter) = script_with_interpreter_len(200);
            let shebang = parse(&script, options).unwrap().unwrap();
            assert_eq!(shebang.interpreter, interpreter.as_slice());
        }
    }

    #[test]
    fn macos_and_linux_line_limits_and_delimiters() {
        let (script, interpreter) = script_with_interpreter_len(509);
        assert_eq!(
            parse(&script, ParseShebangOptions::macos()).unwrap().unwrap().interpreter,
            interpreter.as_slice()
        );
        let (script, _) = script_with_interpreter_len(510);
        assert_eq!(parse(&script, ParseShebangOptions::macos()).unwrap_err(), Error::NOEXEC);
        assert_eq!(parse(b"#!/bin/sh", ParseShebangOptions::macos()).unwrap_err(), Error::NOEXEC);

        let (script, interpreter) = script_with_interpreter_len(253);
        assert_eq!(
            parse(&script, ParseShebangOptions::linux()).unwrap().unwrap().interpreter,
            interpreter.as_slice()
        );
        let (script, _) = script_with_interpreter_len(254);
        assert_eq!(parse(&script, ParseShebangOptions::linux()).unwrap_err(), Error::NOEXEC);
        assert_eq!(
            parse(b"#!/bin/sh -e", ParseShebangOptions::linux()).unwrap().unwrap().arguments,
            [b"-e"]
        );
        assert_eq!(parse(b"#! \n", ParseShebangOptions::linux()).unwrap_err(), Error::NOEXEC);
        assert!(parse(b"\x7fELF", ParseShebangOptions::linux()).unwrap().is_none());

        // Linux & XNU stop at NUL and only trim space/tab (NOT `\r`).
        let crlf = parse(b"#!/bin/sh\r\n", ParseShebangOptions::linux()).unwrap().unwrap();
        assert_eq!(crlf.interpreter.as_slice(), b"/bin/sh\r");
        let crlf_arg = parse(b"#!/bin/sh -e\r\n", ParseShebangOptions::linux()).unwrap().unwrap();
        assert_eq!(crlf_arg.interpreter.as_slice(), b"/bin/sh");
        assert_eq!(crlf_arg.arguments[0].as_slice(), b"-e\r");
        let nul = parse(b"#!/bin/sh\0ignored\n", ParseShebangOptions::linux()).unwrap().unwrap();
        assert_eq!(nul.interpreter.as_slice(), b"/bin/sh");
    }

    #[test]
    fn apply_shebang_handles_empty_and_non_empty_args() {
        let shebang = Shebang {
            interpreter: to_vec_in(b"/bin/sh", Global),
            arguments: {
                let mut v = Vec::new_in(Global);
                v.push(to_vec_in(b"-e", Global));
                v
            },
        };
        let mut exec = Exec {
            program: to_vec_in(b"/tmp/s.sh", Global),
            args: Vec::new_in(Global),
            envs: Vec::new_in(Global),
        };
        apply_shebang(&mut exec, shebang.clone(), to_vec_in(b"/tmp/s.sh", Global));
        assert_eq!(exec.program.as_slice(), b"/bin/sh");
        assert_eq!(
            exec.args.iter().map(Vec::as_slice).collect::<std::vec::Vec<_>>(),
            vec![b"/bin/sh".as_slice(), b"-e", b"/tmp/s.sh"]
        );

        let mut args = Vec::new_in(Global);
        args.push(to_vec_in(b"orig0", Global));
        args.push(to_vec_in(b"a1", Global));
        let mut exec =
            Exec { program: to_vec_in(b"/tmp/s.sh", Global), args, envs: Vec::new_in(Global) };
        apply_shebang(&mut exec, shebang, to_vec_in(b"/tmp/s.sh", Global));
        assert_eq!(
            exec.args.iter().map(Vec::as_slice).collect::<std::vec::Vec<_>>(),
            vec![b"/bin/sh".as_slice(), b"-e", b"/tmp/s.sh", b"a1"]
        );
    }
}
