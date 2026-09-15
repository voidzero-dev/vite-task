use core::{ptr, ptr::NonNull, slice};

use fspy_nostd::{
    BorrowedFd,
    mm::{self, MapFlags, ProtFlags},
};

pub const fn add(left: usize, right: usize) -> usize {
    left.checked_add(right).expect("ELF address overflow")
}

pub const fn sub(left: usize, right: usize) -> usize {
    left.checked_sub(right).expect("invalid ELF address range")
}

pub const fn align_up(value: usize, alignment: usize) -> usize {
    assert!(alignment.is_power_of_two(), "invalid ELF alignment");
    let mask = sub(alignment, 1);
    add(value, mask) & !mask
}

pub fn word(value: u64) -> usize {
    usize::try_from(value).expect("ELF value exceeds native word")
}

/// An owned address range: a parsing view, image reservation, or new stack.
///
/// Drop releases the range during loading or unwinding. Retain hands its
/// lifetime to the target; the kernel reclaims it on a later exec or exit.
pub struct Mapping {
    pub ptr: NonNull<u8>,
    pub len: usize,
}

impl Mapping {
    pub fn reserve(len: usize) -> Self {
        Self::check_len(len);
        // PROT_NONE reserves addresses but permits no access. Image segments
        // later replace its pages; the stack makes its usable pages writable.
        // SAFETY: no fixed address is requested and no references exist yet.
        let ptr = unsafe {
            mm::mmap_anonymous(ptr::null_mut(), len, ProtFlags::empty(), MapFlags::PRIVATE)
        }
        .expect("cannot reserve address space");
        Self::new(ptr.cast(), len)
    }

    pub fn file(fd: BorrowedFd<'_>, len: usize) -> Self {
        Self::check_len(len);
        // One contiguous view makes file-offset parsing possible. This is
        // separate from the segments mapped at their final execution addresses.
        // SAFETY: private read-only view at a kernel-selected address. The
        // exec_elf contract keeps the file unchanged while this view is used.
        let ptr =
            unsafe { mm::mmap(ptr::null_mut(), len, ProtFlags::READ, MapFlags::PRIVATE, fd, 0) }
                .expect("cannot map ELF metadata");
        Self::new(ptr.cast(), len)
    }

    fn check_len(len: usize) {
        assert!(len != 0 && isize::try_from(len).is_ok(), "invalid mapping length");
    }

    fn new(ptr: *mut u8, len: usize) -> Self {
        let Some(ptr) = NonNull::new(ptr) else {
            // SAFETY: this successful mapping is owned here, but address zero
            // cannot back Rust references, so release it before failing.
            let _ = unsafe { mm::munmap(ptr.cast(), len) };
            panic!("mapping returned address zero");
        };
        Self { ptr, len }
    }

    /// Only called on immutable, fully readable file mappings.
    pub const unsafe fn bytes(&self) -> &[u8] {
        // SAFETY: caller establishes readability; length and non-nullness
        // were checked when creating the mapping.
        unsafe { slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }

    pub const fn retain(self) -> *mut u8 {
        // Switching SP or jumping does not transfer Rust ownership. Suppress
        // Drop here so it cannot release memory needed after the handoff.
        let ptr = self.ptr.as_ptr();
        core::mem::forget(self);
        ptr
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: this object owns the whole mapping, including any segment
        // mappings replacing its reserved pages. No target has entered it.
        let _ = unsafe { mm::munmap(self.ptr.as_ptr().cast(), self.len) };
    }
}
