use core::{ffi::c_char, ptr::NonNull, slice};

use bstr::BStr;

use crate::{CStr, Thin, env::Entry};

#[derive(Clone)]
struct PointerIter {
    current: *const *const c_char,
}

impl Iterator for PointerIter {
    type Item = CStr<'static, Thin>;

    fn next(&mut self) -> Option<Self::Item> {
        // SAFETY: the constructor's caller guarantees that the pointer array
        // remains valid and ends in a null pointer.
        let entry = NonNull::new(unsafe { self.current.read() }.cast_mut())?;

        // SAFETY: another pointer slot, possibly the terminating null pointer,
        // follows every non-null entry.
        self.current = unsafe { self.current.add(1) };
        // SAFETY: every non-null pointer in these arrays names an immutable,
        // NUL-terminated string under the constructor's caller contract.
        Some(unsafe { CStr::<Thin>::from_non_null(entry) })
    }
}

/// An iterator over process arguments as thin C strings.
#[derive(Clone)]
pub struct ThinArgs {
    inner: PointerIter,
}

impl Iterator for ThinArgs {
    type Item = CStr<'static, Thin>;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }
}

/// An iterator over process environment entries as thin C strings.
#[derive(Clone)]
pub struct ThinEnvs {
    inner: PointerIter,
}

impl Iterator for ThinEnvs {
    type Item = Entry<Thin>;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(split_thin)
    }
}

// Splitting cannot fail because `entry` is already a valid C string.
fn split_thin(entry: CStr<'static, Thin>) -> Entry<Thin> {
    let start = entry.as_ptr().cast::<u8>();
    let mut len = 0usize;

    for byte in entry.bytes() {
        match byte {
            b'=' => {
                // SAFETY: the scan established the name prefix.
                let name: &'static [u8] = unsafe { slice::from_raw_parts(start, len) };
                // SAFETY: `=` precedes the NUL, so the value is a C string.
                let value = unsafe { CStr::<Thin>::from_ptr(start.add(len).add(1).cast()) };
                return (BStr::new(name), Some(value));
            }
            _ => {
                // A valid C string cannot be `usize::MAX` bytes long.
                len += 1;
            }
        }
    }

    // SAFETY: `Bytes` stopped at the NUL after this prefix.
    let name: &'static [u8] = unsafe { slice::from_raw_parts(start, len) };
    (BStr::new(name), None)
}

/// Returns direct thin C-string views of the macOS process arguments.
///
/// This snapshots the pointer currently exposed by `_NSGetArgv`; it does not
/// count the strings or allocate.
///
/// # Safety
///
/// Until the iterator and every view yielded from it are discarded, the
/// caller must ensure that the argument pointer array and strings remain
/// mapped, readable, and immutable and that no new image is executed.
#[must_use]
pub unsafe fn args() -> ThinArgs {
    // SAFETY: `_NSGetArgv` returns the address of the live argument pointer,
    // and the caller accepts responsibility for keeping it stable.
    let first = unsafe { read_array(libc::_NSGetArgv()) };
    ThinArgs { inner: PointerIter { current: first } }
}

/// Returns direct thin C-string views of the macOS process environment.
///
/// This snapshots the pointer currently exposed by `_NSGetEnviron`; it does
/// not count the strings or allocate.
///
/// # Safety
///
/// Until the iterator and every view yielded from it are discarded, the
/// caller must ensure that no thread mutates the environment or executes a new
/// image. Such operations may replace the pointer array or its strings.
#[must_use]
pub unsafe fn envs() -> ThinEnvs {
    // SAFETY: `_NSGetEnviron` returns the address of the live environment
    // pointer, and the caller accepts responsibility for keeping it stable.
    let first = unsafe { read_array(libc::_NSGetEnviron()) };
    ThinEnvs { inner: PointerIter { current: first } }
}

const unsafe fn read_array(location: *mut *mut *mut c_char) -> *const *const c_char {
    // SAFETY: upheld by the direct iterator constructors' contracts and the
    // guarantees of the Apple accessor functions.
    unsafe { location.read() }.cast_const().cast()
}

#[cfg(test)]
mod tests {
    use bstr::ByteSlice as _;

    use super::*;

    #[test]
    fn thin_entries_distinguish_missing_and_empty_values() {
        // SAFETY: both literals are NUL-terminated and live for the views.
        let missing = unsafe { CStr::<Thin>::from_ptr(c"INVALID".as_ptr()) };
        // SAFETY: as above.
        let empty = unsafe { CStr::<Thin>::from_ptr(c"EMPTY=".as_ptr()) };

        let (name, value) = split_thin(missing);
        assert_eq!(name.as_bytes(), b"INVALID");
        assert!(value.is_none());

        let (name, value) = split_thin(empty);
        assert_eq!(name.as_bytes(), b"EMPTY");
        assert_eq!(value.unwrap().count().as_bytes(), b"");
    }

    #[test]
    fn thin_iterators_contain_argv_zero_and_path() {
        // SAFETY: this test does not mutate the argument or environment arrays
        // while their iterators or borrowed entries are live.
        let argv_zero = unsafe { args() }.next().unwrap().count();
        assert_eq!(argv_zero.as_bytes(), std::env::args_os().next().unwrap().as_encoded_bytes());

        // SAFETY: as above.
        let path = unsafe { envs() }
            .find(|(name, _)| name.as_bytes() == b"PATH")
            .unwrap()
            .1
            .unwrap()
            .count();
        assert_eq!(path.as_bytes(), std::env::var_os("PATH").unwrap().as_encoded_bytes());
    }
}
