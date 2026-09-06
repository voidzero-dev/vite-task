use core::slice;

use crate::{CStr, Thin};

/// A native-word entry in the Linux auxiliary vector.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct AuxEntry {
    pub key: usize,
    pub value: usize,
}

const _: () = {
    use core::mem::{align_of, offset_of, size_of};
    assert!(size_of::<AuxEntry>() == 2 * size_of::<usize>());
    assert!(align_of::<AuxEntry>() == align_of::<usize>());
    assert!(offset_of!(AuxEntry, key) == 0);
    assert!(offset_of!(AuxEntry, value) == size_of::<usize>());
};

/// An immutable view of the stack supplied at Linux executable entry.
///
/// Locates argument pointers, environment pointers, and auxiliary entries
/// without allocating or counting string lengths. The slices borrow the stack
/// directly. Dropping the view does not release the stack or its contents.
pub struct InitialStack<'a> {
    arguments: &'a [CStr<'a, Thin>],
    environment: &'a [CStr<'a, Thin>],
    auxiliary: &'a [AuxEntry],
}

impl<'a> InitialStack<'a> {
    /// Borrows the original stack captured by the executable's entry trampoline.
    ///
    /// # Safety
    ///
    /// `sp` must be aligned and point to a readable Linux initial stack in one
    /// allocation: argc, argc non-null argument pointers, a null pointer,
    /// environment pointers ending in null, and auxiliary pairs ending in
    /// `AT_NULL`. The complete region must fit in `isize::MAX` bytes.
    ///
    /// Every string pointer must name an immutable NUL-terminated string.
    /// Stack storage, strings, and memory referenced by auxiliary entries must
    /// remain valid and unchanged for `'a`. String pointers must carry valid
    /// provenance for their strings.
    /// This reads the entry stack, not the current stack of a Rust function.
    #[must_use]
    pub const unsafe fn from_ptr(sp: *const usize) -> Self {
        // SAFETY: the caller supplies argc followed by that many pointers and
        // a terminator, all within the initial-stack allocation. Thin CStr
        // has pointer layout; the counted entries are non-null valid strings.
        let (arguments, environment) = unsafe {
            let count = sp.read();
            let start = sp.add(1).cast::<*const u8>();
            (slice::from_raw_parts(start.cast::<CStr<'a, Thin>>(), count), start.add(count).add(1))
        };
        let mut count = 0;
        // SAFETY: environment pointers are readable through their terminator.
        while !unsafe { environment.add(count).read() }.is_null() {
            count += 1;
        }
        // SAFETY: the scan found the entire environment region; auxiliary
        // pairs follow its terminator and have the same word alignment. The
        // terminator is excluded before borrowing entries as non-null CStrs.
        let (environment, auxiliary) = unsafe {
            (
                slice::from_raw_parts(environment.cast::<CStr<'a, Thin>>(), count),
                environment.add(count).add(1).cast::<AuxEntry>(),
            )
        };
        let mut count = 0;
        // SAFETY: auxiliary pairs are readable through AT_NULL, whose key is 0.
        while unsafe { auxiliary.add(count).read().key } != 0 {
            count += 1;
        }
        // SAFETY: the scan found all non-terminating pairs in the allocation.
        let auxiliary = unsafe { slice::from_raw_parts(auxiliary, count) };
        Self { arguments, environment, auxiliary }
    }

    /// Borrows arguments, including argv[0], without the null terminator.
    #[must_use]
    pub const fn args(&self) -> &'a [CStr<'a, Thin>] {
        self.arguments
    }

    /// Borrows complete environment strings without the null terminator.
    ///
    /// Entries are preserved verbatim, including entries without `=`.
    #[must_use]
    pub const fn envs(&self) -> &'a [CStr<'a, Thin>] {
        self.environment
    }

    /// Borrows auxiliary key/value pairs, excluding `AT_NULL`.
    ///
    /// Values are unchanged and may contain pointers into the original stack.
    #[must_use]
    pub const fn auxv(&self) -> &'a [AuxEntry] {
        self.auxiliary
    }
}

#[cfg(test)]
mod tests {
    use core::ptr;

    use super::*;

    #[repr(C)]
    struct Stack<const A: usize, const E: usize, const V: usize> {
        count: usize,
        arguments: [*const u8; A],
        arguments_end: *const u8,
        environment: [*const u8; E],
        environment_end: *const u8,
        auxiliary: [AuxEntry; V],
    }

    #[test]
    fn preserves_strings_and_auxiliary_values() {
        let random = [42_u8; 16];
        let raw = Stack {
            count: 3,
            arguments: [
                c"custom-argv-zero".as_ptr().cast(),
                c"".as_ptr().cast(),
                c"\xff".as_ptr().cast(),
            ],
            arguments_end: ptr::null(),
            environment: [
                c"KEY=value=tail".as_ptr().cast(),
                c"EMPTY=".as_ptr().cast(),
                c"NO_EQUALS".as_ptr().cast(),
                c"".as_ptr().cast(),
            ],
            environment_end: ptr::null(),
            auxiliary: [
                AuxEntry { key: 6, value: 4096 },
                AuxEntry { key: 25, value: random.as_ptr().expose_provenance() },
                AuxEntry { key: 0x1234, value: 0 },
                AuxEntry { key: 0, value: 0 },
                AuxEntry { key: 99, value: 99 },
            ],
        };
        // SAFETY: raw models the Linux layout; all strings and auxiliary
        // data remain readable and immutable until the view is discarded.
        let stack = unsafe { InitialStack::from_ptr(ptr::from_ref(&raw).cast()) };
        assert_eq!(stack.args().len(), 3);
        for (arg, expected) in
            stack.args().iter().zip([b"custom-argv-zero".as_slice(), b"", b"\xff"])
        {
            assert_eq!(arg.count().as_units(), expected);
        }
        assert_eq!(stack.envs().len(), 4);
        for (env, expected) in
            stack.envs().iter().zip([b"KEY=value=tail".as_slice(), b"EMPTY=", b"NO_EQUALS", b""])
        {
            assert_eq!(env.count().as_units(), expected);
        }
        assert_eq!(stack.auxv(), &raw.auxiliary[..3]);
        // The arrays and their strings are borrowed, not copied.
        assert_eq!(stack.args()[0].as_ptr(), raw.arguments[0]);
        assert_eq!(stack.args().as_ptr().cast(), raw.arguments.as_ptr());
        assert_eq!(stack.envs().as_ptr().cast(), raw.environment.as_ptr());
        assert_eq!(stack.auxv().as_ptr(), raw.auxiliary.as_ptr());
    }

    #[test]
    fn accepts_empty_regions() {
        let raw = Stack {
            count: 0,
            arguments: [],
            arguments_end: ptr::null(),
            environment: [],
            environment_end: ptr::null(),
            auxiliary: [AuxEntry { key: 0, value: 0 }],
        };
        // SAFETY: argc is zero, both pointer arrays and auxv end immediately.
        let stack = unsafe { InitialStack::from_ptr(ptr::from_ref(&raw).cast()) };
        assert!(stack.args().is_empty());
        assert!(stack.envs().is_empty());
        assert!(stack.auxv().is_empty());
    }

    #[test]
    fn slices_borrow_storage_independently_of_view() {
        let raw = Stack {
            count: 1,
            arguments: [c"arg".as_ptr().cast()],
            arguments_end: ptr::null(),
            environment: [],
            environment_end: ptr::null(),
            auxiliary: [AuxEntry { key: 0, value: 0 }],
        };
        let args = {
            // SAFETY: backing storage and literal outlive the returned slice.
            let stack = unsafe { InitialStack::from_ptr(ptr::from_ref(&raw).cast()) };
            stack.args()
        };
        assert_eq!(args.len(), 1);
        assert_eq!(args[0].count().as_units(), b"arg");
    }
}
