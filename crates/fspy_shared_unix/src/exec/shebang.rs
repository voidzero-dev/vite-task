use std::path::Path;

use bstr::{BString, ByteSlice};

#[derive(Debug, Clone)]
pub struct Shebang {
    pub interpreter: BString,
    pub arguments: Vec<BString>,
}

const fn is_whitespace(c: u8) -> bool {
    c == b' ' || c == b'\t'
}

#[derive(Clone, Copy, Debug)]
pub struct ParseShebangOptions {
    pub split_arguments: bool, // TODO: recursive
    /// How many bytes of the file the kernel reads to find the shebang line.
    pub max_line_len: usize,
    /// Whether the line must end with a newline within `max_line_len` bytes.
    ///
    /// macOS refuses a script without one. Linux accepts it as long as the
    /// interpreter path is followed by a space or tab, so only the
    /// arguments can have been cut off.
    pub require_newline: bool,
}

/// Size of the buffer the macOS kernel reads the shebang line into (`IMG_SHSIZE`).
const MACOS_MAX_LINE_LEN: usize = 512;
/// Size of the buffer the Linux kernel reads the shebang line into (`BINPRM_BUF_SIZE`).
const LINUX_MAX_LINE_LEN: usize = 256;

impl ParseShebangOptions {
    #[must_use]
    pub const fn macos() -> Self {
        Self { split_arguments: true, max_line_len: MACOS_MAX_LINE_LEN, require_newline: true }
    }

    #[must_use]
    pub const fn linux() -> Self {
        Self { split_arguments: false, max_line_len: LINUX_MAX_LINE_LEN, require_newline: false }
    }
}

impl Default for ParseShebangOptions {
    fn default() -> Self {
        if cfg!(target_vendor = "apple") { Self::macos() } else { Self::linux() }
    }
}

/// Parses the shebang line of the executable at `path` the way the kernel does.
///
/// Returns `ENOEXEC` where the kernel would refuse the script instead of
/// running a truncated interpreter path.
///
/// # Panics
///
/// Panics if `options.max_line_len` is larger than 512.
pub fn parse_shebang(
    mut peek_executable: impl FnMut(&Path, &mut [u8]) -> nix::Result<usize>,
    path: &Path,
    options: ParseShebangOptions,
) -> Result<Option<Shebang>, nix::Error> {
    let mut buf = [0u8; MACOS_MAX_LINE_LEN];
    let buf = &mut buf[..options.max_line_len];

    let total_read_size = peek_executable(path, buf)?;
    let buf = &buf[..total_read_size];

    let Some(line) = buf.strip_prefix(b"#!") else {
        return Ok(None);
    };

    let line = if let Some(newline) = line.find_byte(b'\n') {
        &line[..newline]
    } else if options.require_newline {
        return Err(nix::Error::ENOEXEC);
    } else if total_read_size < options.max_line_len {
        // The whole file fits in the buffer, so nothing was cut off.
        line
    } else {
        // https://github.com/torvalds/linux/blob/v6.12/fs/binfmt_script.c
        // Like the kernel, keep the last byte of the buffer as a terminator.
        let line = &line[..line.len() - 1];
        // The interpreter path must be followed by a space or tab, or it may be cut off.
        if !line.iter().skip_while(|ch| is_whitespace(**ch)).any(|ch| is_whitespace(*ch)) {
            return Err(nix::Error::ENOEXEC);
        }
        line
    };

    let line = line.trim_ascii();
    let interpreter = line.split(|ch| is_whitespace(*ch)).next().unwrap_or_default();
    if interpreter.is_empty() {
        return Err(nix::Error::ENOEXEC);
    }
    let arguments_buf = line[interpreter.len()..].trim_ascii_start().as_bstr();

    let arguments: Vec<BString> = if options.split_arguments {
        arguments_buf
            .split(|ch| is_whitespace(*ch))
            .filter_map(|arg| {
                let arg = arg.trim_ascii();
                if arg.is_empty() { None } else { Some(arg.as_bstr().to_owned()) }
            })
            .collect()
    } else if arguments_buf.is_empty() {
        vec![]
    } else {
        vec![arguments_buf.to_owned()]
    };

    Ok(Some(Shebang { interpreter: interpreter.as_bstr().to_owned(), arguments }))
}

// #[derive(Debug)]
// pub struct RecursiveParseOpts {
//     pub recursion_limit: usize,
//     pub split_arguments: bool,
// }

// impl Default for RecursiveParseOpts {
//     fn default() -> Self {
//         Self {
//             recursion_limit: 4, // BINPRM_MAX_RECURSION
//             split_arguments: false,
//         }
//     }
// }

// fn parse_shebang_recursive_impl<R: Read>(
//     buf: &mut [u8],
//     reader: R,
//     mut get_reader: impl FnMut(&OsStr) -> io::Result<R>,
//     mut on_shebang: impl FnMut(shebang<'_>) -> io::Result<()>,
// ) -> io::Result<()> {
//     let Some(mut shebang) = parse_shebang(buf, reader)? else {
//         return Ok(());
//     };
//     on_shebang(shebang)?;
//     loop {
//         let reader = get_reader(&shebang.interpreter)?;
//         let Some(cur_shebang) = parse_shebang(buf, reader)? else {
//             break Ok(());
//         };
//         on_shebang(cur_shebang)?;
//         shebang = cur_shebang;
//     }
// }

// pub fn parse_shebang_recursive<
//     const PEEK_CAP: usize,
//     R: Read,
//     O: FnMut(&OsStr) -> io::Result<R>,
//     C: FnMut(&OsStr) -> io::Result<()>,
// >(
//     opts: RecursiveParseOpts,
//     reader: R,
//     open: O,
//     mut on_arg_reverse: C,
// ) -> io::Result<()> {
//     let mut peek_buf = [0u8; PEEK_CAP];
//     let mut recursive_count = 0;
//     parse_shebang_recursive_impl(&mut peek_buf, reader, open, |shebang| {
//         if recursive_count > opts.recursion_limit {
//             return Err(io::Error::from_raw_os_error(libc::ELOOP));
//         }
//         if opts.split_arguments {
//             for arg in shebang.arguments.split().rev() {
//                 on_arg_reverse(arg)?;
//             }
//         } else {
//             on_arg_reverse(shebang.arguments.as_one())?;
//         }
//         on_arg_reverse(shebang.interpreter)?;
//         recursive_count += 1;
//         Ok(())
//     })?;
//     Ok(())
// }

// #[cfg(test)]
// mod tests {
//     use std::os::unix::ffi::OsStrExt;

//     use super::*;

//     #[test]
//     fn shebang_basic() {
//         let mut buf = [0u8; PEEK_SIZE];
//         let shebang = parse_shebang(&mut buf, "#!/bin/sh a b\n".as_bytes())
//             .unwrap()
//             .unwrap();
//         assert_eq!(shebang.interpreter.as_bytes(), b"/bin/sh");
//         assert_eq!(shebang.arguments.as_one().as_bytes(), b"a b");
//         assert_eq!(
//             shebang
//                 .arguments
//                 .split()
//                 .map(OsStrExt::as_bytes)
//                 .collect::<Vec<_>>(),
//             vec![b"a", b"b"]
//         );
//     }

//     #[test]
//     fn shebang_trimming_spaces() {
//         let mut buf = [0u8; PEEK_SIZE];
//         let shebang = parse_shebang(&mut buf, "#! /bin/sh a \n".as_bytes())
//             .unwrap()
//             .unwrap();
//         assert_eq!(shebang.interpreter, "/bin/sh");
//         assert_eq!(shebang.arguments.as_one().as_bytes(), b"a");
//         assert_eq!(
//             shebang
//                 .arguments
//                 .split()
//                 .map(OsStrExt::as_bytes)
//                 .collect::<Vec<_>>(),
//             vec![b"a"]
//         );
//     }

//     #[test]
//     fn shebang_split_arguments() {
//         let mut buf = [0u8; PEEK_SIZE];
//         let shebang = parse_shebang(&mut buf, "#! /bin/sh a  b\tc \n".as_bytes())
//             .unwrap()
//             .unwrap();
//         assert_eq!(shebang.interpreter, "/bin/sh");
//         assert_eq!(
//             shebang
//                 .arguments
//                 .split()
//                 .map(OsStrExt::as_bytes)
//                 .collect::<Vec<_>>(),
//             &[b"a", b"b", b"c"]
//         );
//     }
//     #[test]
//     fn shebang_recursive_basic() {
//         let mut args = Vec::<String>::new();
//         parse_shebang_recursive::<PEEK_SIZE, _, _, _>(
//             RecursiveParseOpts {
//                 split_arguments: true,
//                 ..RecursiveParseOpts::default()
//             },
//             "#!/bin/B bparam".as_bytes(),
//             |path| {
//                 Ok(match path.as_bytes() {
//                     b"/bin/B" => "#! /bin/A aparam1 aparam2".as_bytes(),
//                     b"/bin/A" => "not a shebang script".as_bytes(),
//                     _ => unreachable!("Unexpected path: {}", path.display()),
//                 })
//             },
//             |arg| {
//                 args.push(str::from_utf8(arg.as_bytes()).unwrap().to_owned());
//                 Ok(())
//             },
//         )
//         .unwrap();
//         args.reverse();
//         assert_eq!(
//             args,
//             vec!["/bin/A", "aparam1", "aparam2", "/bin/B", "bparam"]
//         );
//     }
// }

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn parse(content: &[u8], options: ParseShebangOptions) -> nix::Result<Option<Shebang>> {
        parse_shebang(
            |_, buf| {
                let len = content.len().min(buf.len());
                buf[..len].copy_from_slice(&content[..len]);
                Ok(len)
            },
            Path::new("/script"),
            options,
        )
    }

    fn script_with_interpreter_len(len: usize) -> (Vec<u8>, Vec<u8>) {
        let interpreter = [b"/".as_slice(), &vec![b'x'; len - 1]].concat();
        let script = [b"#!".as_slice(), &interpreter, b"\necho hi\n"].concat();
        (script, interpreter)
    }

    #[test]
    fn keeps_interpreter_paths_longer_than_128_bytes() {
        for options in [ParseShebangOptions::macos(), ParseShebangOptions::linux()] {
            let (script, interpreter) = script_with_interpreter_len(200);
            let shebang = parse(&script, options).unwrap().unwrap();
            assert_eq!(shebang.interpreter, interpreter);
        }
    }

    #[test]
    fn macos_accepts_line_up_to_512_bytes() {
        // "#!" + 509 bytes + "\n" is exactly 512 bytes.
        let (script, interpreter) = script_with_interpreter_len(509);
        let shebang = parse(&script, ParseShebangOptions::macos()).unwrap().unwrap();
        assert_eq!(shebang.interpreter, interpreter);

        let (script, _) = script_with_interpreter_len(510);
        assert_eq!(parse(&script, ParseShebangOptions::macos()).unwrap_err(), nix::Error::ENOEXEC);
    }

    #[test]
    fn macos_requires_newline() {
        let result = parse(b"#!/bin/sh", ParseShebangOptions::macos());
        assert_eq!(result.unwrap_err(), nix::Error::ENOEXEC);
    }

    #[test]
    fn linux_accepts_line_up_to_256_bytes() {
        // "#!" + 253 bytes + "\n" is exactly 256 bytes.
        let (script, interpreter) = script_with_interpreter_len(253);
        let shebang = parse(&script, ParseShebangOptions::linux()).unwrap().unwrap();
        assert_eq!(shebang.interpreter, interpreter);

        let (script, _) = script_with_interpreter_len(254);
        assert_eq!(parse(&script, ParseShebangOptions::linux()).unwrap_err(), nix::Error::ENOEXEC);
    }

    #[test]
    fn linux_accepts_missing_newline_at_end_of_file() {
        let shebang = parse(b"#!/bin/sh -e", ParseShebangOptions::linux()).unwrap().unwrap();
        assert_eq!(shebang.interpreter, "/bin/sh");
        assert_eq!(shebang.arguments, vec![BString::from("-e")]);
    }

    #[test]
    fn linux_refuses_truncated_interpreter() {
        let (script, _) = script_with_interpreter_len(300);
        assert_eq!(parse(&script, ParseShebangOptions::linux()).unwrap_err(), nix::Error::ENOEXEC);
    }

    #[test]
    fn linux_truncates_long_arguments() {
        let script = [b"#!/bin/sh ".as_slice(), &[b'a'; 300], b"\n"].concat();
        let shebang = parse(&script, ParseShebangOptions::linux()).unwrap().unwrap();
        assert_eq!(shebang.interpreter, "/bin/sh");
        // 256-byte buffer, minus "#!/bin/sh " and the reserved last byte.
        assert_eq!(shebang.arguments, vec![BString::from(vec![b'a'; 256 - 10 - 1])]);
    }

    #[test]
    fn missing_interpreter_is_an_error() {
        for options in [ParseShebangOptions::macos(), ParseShebangOptions::linux()] {
            assert_eq!(parse(b"#!  \n", options).unwrap_err(), nix::Error::ENOEXEC);
        }
    }

    #[test]
    fn not_a_script() {
        assert!(parse(b"\x7fELF", ParseShebangOptions::default()).unwrap().is_none());
    }
}
