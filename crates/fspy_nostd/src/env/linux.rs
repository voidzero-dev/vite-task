use core::{ffi::CStr as CoreCStr, num::NonZeroUsize, ptr, slice};

use atoi::FromRadix10Checked as _;
use bstr::{BStr, ByteSlice as _};
use rustix::fs::{Mode, OFlags};

use super::Entry;
use crate::{CStr, CWD, Error, Fat, Result};

#[derive(Clone, Copy)]
struct Bounds {
    arg_start: usize,
    arg_end: usize,
    env_start: usize,
    env_end: usize,
}

struct RangeIter<'a> {
    remaining: &'a [u8],
}

impl<'a> RangeIter<'a> {
    const fn new(range: &'a [u8]) -> Self {
        Self { remaining: range }
    }
}

impl<'a> Iterator for RangeIter<'a> {
    type Item = CStr<'a, Fat>;

    fn next(&mut self) -> Option<Self::Item> {
        let Some(nul) = self.remaining.iter().position(|byte| *byte == 0) else {
            self.remaining = &[];
            return None;
        };
        let entry_len = NonZeroUsize::new(nul.checked_add(1)?)?;
        let (entry, remaining) = self.remaining.split_at_checked(entry_len.get())?;
        self.remaining = remaining;

        // SAFETY: splitting at the first NUL produces a nonempty slice with
        // exactly one trailing NUL.
        Some(unsafe { CStr::from_units_with_nul_unchecked(entry) })
    }
}

fn split_fat(entry: CStr<'static, Fat>) -> Entry {
    let Some((name, value)) = entry.as_units_with_nul().split_once_str(b"=") else {
        return (BStr::new(entry.as_units()), None);
    };

    // SAFETY: splitting preserves the single trailing NUL.
    let value = unsafe { CStr::from_units_with_nul_unchecked(value) };
    (BStr::new(name), Some(value))
}

/// A snapshot of the process argument and environment string ranges.
pub struct Current {
    args: &'static [u8],
    envs: &'static [u8],
}

impl Current {
    /// Converts the kernel-published address bounds into borrowed slices.
    ///
    /// # Safety
    ///
    /// Both ranges must be immutable and readable for the lifetime of every
    /// returned view.
    const unsafe fn from_bounds(bounds: Bounds) -> Result<Self> {
        // SAFETY: upheld by this function's caller.
        let args = match unsafe { slice_from_range(bounds.arg_start, bounds.arg_end) } {
            Ok(args) => args,
            Err(error) => return Err(error),
        };
        // SAFETY: upheld by this function's caller.
        let envs = match unsafe { slice_from_range(bounds.env_start, bounds.env_end) } {
            Ok(envs) => envs,
            Err(error) => return Err(error),
        };
        Ok(Self { args, envs })
    }

    /// Returns a fresh iterator over the process arguments.
    #[must_use]
    pub const fn args(&self) -> FatArgs {
        FatArgs { inner: RangeIter::new(self.args) }
    }

    /// Returns a fresh iterator over the process environment.
    #[must_use]
    pub const fn envs(&self) -> FatEnvs {
        FatEnvs { inner: RangeIter::new(self.envs) }
    }
}

/// Converts one kernel-published address range into a borrowed slice.
///
/// # Safety
///
/// The nonempty range must belong to one immutable, readable allocation that
/// remains valid for `'a`.
const unsafe fn slice_from_range<'a>(start: usize, end: usize) -> Result<&'a [u8]> {
    let len = match end.checked_sub(start) {
        Some(len) if len <= isize::MAX.cast_unsigned() => len,
        _ => return Err(Error::INVAL),
    };
    let Some(len) = NonZeroUsize::new(len) else {
        return Ok(&[]);
    };
    let Some(start) = NonZeroUsize::new(start) else {
        return Err(Error::INVAL);
    };

    let start = ptr::with_exposed_provenance::<u8>(start.get());
    // SAFETY: the caller guarantees that the complete address range is one
    // immutable, readable allocation for the requested lifetime.
    Ok(unsafe { slice::from_raw_parts(start, len.get()) })
}

/// An iterator over process arguments as counted C strings.
pub struct FatArgs {
    inner: RangeIter<'static>,
}

impl Iterator for FatArgs {
    type Item = CStr<'static, Fat>;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }
}

/// An iterator over process environment entries as counted C strings.
pub struct FatEnvs {
    inner: RangeIter<'static>,
}

impl Iterator for FatEnvs {
    type Item = Entry;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(split_fat)
    }
}

/// Snapshots the process argument and environment ranges.
///
/// This opens and reads `/proc/self/stat` once into fixed stack storage and
/// parses `arg_start`, `arg_end`, `env_start`, and `env_end` together. It does
/// not allocate, and rustix's raw Linux backend makes the file operations
/// direct syscalls. Iterators subsequently created from the snapshot perform
/// no syscalls.
///
/// # Errors
///
/// Returns [`crate::Error`] if `/proc/self/stat` cannot be opened or read, does
/// not fit in the fixed buffer, or does not contain valid string bounds.
///
/// # Safety
///
/// Until the snapshot and every view yielded from it are discarded, the
/// caller must ensure that its published argument and environment ranges
/// remain mapped, readable, and immutable. In particular, no thread may
/// rewrite the process title or original environment strings, change the
/// bounds with `PR_SET_MM`, or execute a new image.
pub unsafe fn current() -> Result<Current> {
    let bounds = read_bounds()?;
    // SAFETY: the caller guarantees that both published address ranges remain
    // valid, readable, and immutable for every returned view.
    unsafe { Current::from_bounds(bounds) }
}

fn read_bounds() -> Result<Bounds> {
    const STAT_PATH: &CoreCStr = c"/proc/self/stat";
    const STAT_CAPACITY: usize = 4096;

    let fd = rustix::fs::openat(CWD, STAT_PATH, OFlags::RDONLY | OFlags::CLOEXEC, Mode::empty())?;
    let mut stat = [0; STAT_CAPACITY];
    let mut initialized = 0;

    loop {
        let Some(remaining) = stat.get_mut(initialized..) else {
            return Err(Error::OVERFLOW);
        };
        if remaining.is_empty() {
            return Err(Error::OVERFLOW);
        }

        let Some(read) = NonZeroUsize::new(rustix::io::read(&fd, remaining)?) else {
            break;
        };
        initialized = initialized.checked_add(read.get()).ok_or(Error::OVERFLOW)?;
    }

    parse_bounds(&stat[..initialized])
}

fn parse_bounds(stat: &[u8]) -> Result<Bounds> {
    // `comm` (field 2) may itself contain spaces, newlines, and `)`. The
    // kernel-added delimiter is the last `)` because every later field is
    // numeric except for the one-byte process state.
    let comm_end = stat.iter().rposition(|byte| *byte == b')').ok_or(Error::INVAL)?;
    let (_, comm_and_fields) = stat.split_at_checked(comm_end).ok_or(Error::INVAL)?;
    let (_, fields) = comm_and_fields.split_first().ok_or(Error::INVAL)?;
    let mut fields = fields.split(u8::is_ascii_whitespace).filter(|field| !field.is_empty());

    // With field 3 at index zero, arg_start (field 48) is index 45, followed
    // by arg_end, env_start, and env_end.
    let arg_start = fields.nth(45).ok_or(Error::INVAL)?;
    let arg_end = fields.next().ok_or(Error::INVAL)?;
    let env_start = fields.next().ok_or(Error::INVAL)?;
    let env_end = fields.next().ok_or(Error::INVAL)?;
    Ok(Bounds {
        arg_start: parse_usize(arg_start)?,
        arg_end: parse_usize(arg_end)?,
        env_start: parse_usize(env_start)?,
        env_end: parse_usize(env_end)?,
    })
}

fn parse_usize(bytes: &[u8]) -> Result<usize> {
    let (value, used) = usize::from_radix_10_checked(bytes);
    let value = value.ok_or(Error::OVERFLOW)?;
    let used = NonZeroUsize::new(used).ok_or(Error::INVAL)?;
    if used.get() != bytes.len() {
        return Err(Error::INVAL);
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iterates_argument_and_environment_ranges() {
        static ARGS: &[u8] = b"program\0--flag\0";
        static ENVS: &[u8] = b"FIRST=one\0INVALID\0EMPTY=\0LAST=a=b\0";
        let current = Current { args: ARGS, envs: ENVS };

        let mut args = current.args();
        assert_eq!(args.next().unwrap().as_units(), b"program");
        assert_eq!(args.next().unwrap().as_units(), b"--flag");
        assert!(args.next().is_none());

        let mut envs = current.envs();
        let (name, value) = envs.next().unwrap();
        assert_eq!(name.as_bytes(), b"FIRST");
        assert_eq!(value.unwrap().as_units(), b"one");

        let (name, value) = envs.next().unwrap();
        assert_eq!(name.as_bytes(), b"INVALID");
        assert!(value.is_none());

        let (name, value) = envs.next().unwrap();
        assert_eq!(name.as_bytes(), b"EMPTY");
        assert_eq!(value.unwrap().as_units(), b"");

        let (name, value) = envs.next().unwrap();
        assert_eq!(name.as_bytes(), b"LAST");
        assert_eq!(value.unwrap().as_units(), b"a=b");
        assert!(envs.next().is_none());
    }

    #[test]
    fn entry_splits_only_at_the_first_equals() {
        // SAFETY: this static byte string contains exactly one trailing NUL.
        let entry = unsafe { CStr::<Fat>::from_units_with_nul_unchecked(b"NAME=a=b\0") };
        let (name, value) = split_fat(entry);
        let value = value.unwrap();

        assert_eq!(name.as_bytes(), b"NAME");
        assert_eq!(value.as_units(), b"a=b");
        assert_eq!(value.as_units_with_nul(), b"a=b\0");
    }

    #[test]
    fn entry_accepts_an_empty_value() {
        // SAFETY: this static byte string contains exactly one trailing NUL.
        let entry = unsafe { CStr::<Fat>::from_units_with_nul_unchecked(b"EMPTY=\0") };
        let (name, value) = split_fat(entry);
        let value = value.unwrap();

        assert_eq!(name.as_bytes(), b"EMPTY");
        assert_eq!(value.as_units_with_nul(), b"\0");
    }

    #[test]
    fn entry_without_equals_has_no_value() {
        // SAFETY: this static byte string contains exactly one trailing NUL.
        let entry = unsafe { CStr::<Fat>::from_units_with_nul_unchecked(b"INVALID\0") };
        let (name, value) = split_fat(entry);

        assert_eq!(name.as_bytes(), b"INVALID");
        assert!(value.is_none());
    }

    #[test]
    fn parses_current_process_bounds() {
        let bounds = read_bounds().unwrap();
        assert!(bounds.arg_start <= bounds.arg_end);
        assert!(bounds.env_start <= bounds.env_end);
    }

    #[test]
    fn parses_bounds_after_a_multiline_comm_containing_a_parenthesis() {
        let mut stat = b"123 (before)\nafter) R".to_vec();
        for _ in 4..48 {
            stat.extend_from_slice(b" 0");
        }
        stat.extend_from_slice(b" 100 200 300 400 0\n");

        let bounds = parse_bounds(&stat).unwrap();
        assert_eq!((bounds.arg_start, bounds.arg_end), (100, 200));
        assert_eq!((bounds.env_start, bounds.env_end), (300, 400));
    }

    #[test]
    fn parses_only_complete_in_range_decimal_fields() {
        assert_eq!(parse_usize(b"42"), Ok(42));
        assert_eq!(parse_usize(b""), Err(Error::INVAL));
        assert_eq!(parse_usize(b"42x"), Err(Error::INVAL));
        assert_eq!(
            parse_usize(b"9999999999999999999999999999999999999999999"),
            Err(Error::OVERFLOW)
        );
    }

    #[test]
    fn process_snapshot_contains_argv_zero_and_path() {
        // SAFETY: this test does not mutate the process arguments or
        // environment while its snapshot or borrowed entries are live.
        let current = unsafe { current().unwrap() };

        let argv_zero = current.args().next().unwrap();
        assert_eq!(argv_zero.as_units(), std::env::args_os().next().unwrap().as_encoded_bytes());

        let path = current.envs().find(|(name, _)| name.as_bytes() == b"PATH").unwrap().1.unwrap();
        assert_eq!(path.as_units(), std::env::var_os("PATH").unwrap().as_encoded_bytes());
    }

    #[test]
    fn rejects_an_unterminated_range() {
        static ARGS: &[u8] = b"program";
        let current = Current { args: ARGS, envs: &[] };
        assert!(current.args().next().is_none());
    }
}
