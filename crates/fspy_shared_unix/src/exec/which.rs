use bstr::{BStr, ByteSlice};
use stackalloc::alloca;

fn concat<R>(s: &[&BStr], callback: impl FnOnce(&BStr) -> R) -> R {
    let size = s.iter().map(|s| s.len()).sum();
    alloca(size, |buf| {
        debug_assert_eq!(buf.len(), size);
        let mut pos = 0usize;
        for s in s {
            let next_pos = pos + s.len();
            let bytes: &[u8] = s.as_ref();
            let src_ptr = bytes.as_ptr();
            let dst_ptr = buf[pos..next_pos].as_mut_ptr().cast::<u8>();
            // SAFETY: `src_ptr` and `dst_ptr` are derived from valid slices of known lengths,
            // they do not overlap (src is from the input slice, dst is from the stack-allocated buffer),
            // and `s.len()` bytes are within bounds for both.
            unsafe {
                std::ptr::copy_nonoverlapping(src_ptr, dst_ptr, s.len());
            }
            pos = next_pos;
        }
        debug_assert_eq!(pos, buf.len());
        // SAFETY: `buf.as_ptr()` points to a valid allocation of `buf.len()` bytes that was
        // fully initialized by the copy loop above (verified by the debug_assert).
        let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), buf.len()) };
        callback(bytes.as_bstr())
    })
}

const NAME_MAX: usize = 255;

/// Search the executable in PATH.
///
/// Referenced musl Implementation: <https://github.com/kraj/musl/blob/1b06420abdf46f7d06ab4067e7c51b8b63731852/src/process/execvp.c#L5>
///
/// Difference from musl:
/// - Instead of actually calling execve, use `access_executable` to check if the file is executable, and call `callback` with the found executable.
/// - The path limit (`PATH_MAX`) is not checked.
/// - PATH is passed as parameter instead of using the real environment variable.
pub fn which<R>(
    file: &BStr,
    path: &BStr,
    mut access_executable: impl FnMut(&BStr) -> nix::Result<()>,
    callback: impl FnOnce(&BStr) -> nix::Result<R>,
) -> nix::Result<R> {
    use nix::Error;
    // 1. If file is empty, return ENOENT
    if file.is_empty() {
        return Err(Error::ENOENT);
    }
    // 2. If file contains '/', call callback directly
    if file.contains(&b'/') {
        return callback(file);
    }
    // 3. If file is too long, return ENAMETOOLONG
    if file.len() > NAME_MAX {
        return Err(Error::ENAMETOOLONG);
    }
    // 4. Search PATH
    let mut seen_eacces = false;
    let mut last_err = Error::ENOENT;
    let mut callback = Some(callback);
    for p in path.split(|ch| *ch == b':') {
        let p = p.as_bstr();
        let result_to_return = concat(
            // join with '/' if path is not empty
            &[p, (if p.is_empty() { "" } else { "/" }).into(), file],
            |path| match access_executable(path) {
                Ok(()) => Some((callback.take().unwrap())(path)),
                Err(err @ (Error::EACCES | Error::ENOENT | Error::ENOTDIR)) => {
                    if err == Error::EACCES {
                        seen_eacces = true;
                    }
                    last_err = err;
                    None
                }
                Err(other_err) => Some(Err(other_err)),
            },
        );
        if let Some(result) = result_to_return {
            return result;
        }
    }
    Err(if seen_eacces { Error::EACCES } else { last_err })
}

#[cfg(test)]
mod tests {
    use bstr::{B, BStr};

    use super::*;

    #[test]
    fn test_concat() {
        let s = concat(&["a".into(), "bc".into(), "".into(), "e".into()], BStr::to_owned);
        assert_eq!(s, "abce");
    }

    fn mock_access<'a>(
        allowed: &'a [&'a str],
        fail_eacces: &'a [&'a str],
        fail_enotdir: &'a [&'a str],
    ) -> impl FnMut(&BStr) -> nix::Result<()> + 'a {
        move |path| {
            let s = std::str::from_utf8(path).unwrap();
            if allowed.contains(&s) {
                Ok(())
            } else if fail_eacces.contains(&s) {
                Err(nix::Error::EACCES)
            } else if fail_enotdir.contains(&s) {
                Err(nix::Error::ENOTDIR)
            } else {
                Err(nix::Error::ENOENT)
            }
        }
    }

    /// (case, file, PATH, executable paths, EACCES paths, ENOTDIR paths,
    /// the executable found or the error); any other path fails with ENOENT.
    type WhichCase<'a> = (
        &'a str,
        &'a [u8],
        &'a str,
        &'a [&'a str],
        &'a [&'a str],
        &'a [&'a str],
        Result<&'a str, nix::Error>,
    );

    #[test]
    fn test_which() {
        let long_name = vec![b'a'; NAME_MAX + 1];
        let cases: &[WhichCase<'_>] = &[
            ("found", b"foo", "/bin:/usr/bin", &["/bin/foo"], &[], &[], Ok("/bin/foo")),
            ("not found", b"foo", "/bin:/usr/bin", &[], &[], &[], Err(nix::Error::ENOENT)),
            (
                "eacces",
                b"foo",
                "/bin:/usr/bin",
                &[],
                &["/bin/foo", "/usr/bin/foo"],
                &[],
                Err(nix::Error::EACCES),
            ),
            ("enotdir", b"foo", "/usr/bin:/bin", &[], &[], &["/bin/foo"], Err(nix::Error::ENOTDIR)),
            ("slash in file", b"/usr/bin/foo", "", &["/usr/bin/foo"], &[], &[], Ok("/usr/bin/foo")),
            ("empty file", b"", "/bin:/usr/bin", &[], &[], &[], Err(nix::Error::ENOENT)),
            (
                "file too long",
                &long_name,
                "/bin:/usr/bin",
                &[],
                &[],
                &[],
                Err(nix::Error::ENAMETOOLONG),
            ),
            ("empty path entry", b"foo", ":/bin", &["foo"], &[], &[], Ok("foo")),
        ];
        for &(case, file, path, executable, fail_eacces, fail_enotdir, expected) in cases {
            let access = mock_access(executable, fail_eacces, fail_enotdir);
            let res =
                which(B(file).as_bstr(), B(path).as_bstr(), access, |found| Ok(found.to_owned()));
            assert_eq!(res, expected.map(|found| B(found).as_bstr().to_owned()), "{case}");
        }
    }
}
