use allocator_api2::{alloc::Allocator, vec::Vec};
use bstr::{BStr, ByteSlice};

use super::to_vec_in;

const NAME_MAX: usize = 255;

/// Search the executable in PATH, returning its path in `allocator`.
///
/// Referenced musl Implementation: <https://github.com/kraj/musl/blob/1b06420abdf46f7d06ab4067e7c51b8b63731852/src/process/execvp.c#L5>
///
/// Difference from musl:
/// - Instead of actually calling execve, use `access_executable` to check if the file is executable, and return the found executable.
/// - The path limit (`PATH_MAX`) is not checked.
/// - PATH is passed as parameter instead of using the real environment variable.
pub fn which<A: Allocator>(
    file: &BStr,
    path: &BStr,
    mut access_executable: impl FnMut(&BStr) -> nix::Result<()>,
    allocator: A,
) -> nix::Result<Vec<u8, A>> {
    use nix::Error;
    // 1. If file is empty, return ENOENT
    if file.is_empty() {
        return Err(Error::ENOENT);
    }
    // 2. If file contains '/', return it directly
    if file.contains(&b'/') {
        return Ok(to_vec_in(file, allocator));
    }
    // 3. If file is too long, return ENAMETOOLONG
    if file.len() > NAME_MAX {
        return Err(Error::ENAMETOOLONG);
    }
    // 4. Search PATH, building each candidate in the same buffer
    let mut seen_eacces = false;
    let mut last_err = Error::ENOENT;
    let mut candidate = Vec::new_in(allocator);
    for p in path.split(|ch| *ch == b':') {
        candidate.clear();
        candidate.extend_from_slice(p);
        // join with '/' if path is not empty
        if !p.is_empty() {
            candidate.push(b'/');
        }
        candidate.extend_from_slice(file);
        match access_executable(candidate.as_bstr()) {
            Ok(()) => return Ok(candidate),
            Err(err @ (Error::EACCES | Error::ENOENT | Error::ENOTDIR)) => {
                if err == Error::EACCES {
                    seen_eacces = true;
                }
                last_err = err;
            }
            Err(other_err) => return Err(other_err),
        }
    }
    Err(if seen_eacces { Error::EACCES } else { last_err })
}

#[cfg(test)]
mod tests {
    use allocator_api2::alloc::Global;
    use bstr::B;

    use super::*;

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
            let res = which(B(file).as_bstr(), B(path).as_bstr(), access, Global)
                .map(|found| found.as_bstr().to_owned());
            assert_eq!(res, expected.map(|found| B(found).as_bstr().to_owned()), "{case}");
        }
    }
}
