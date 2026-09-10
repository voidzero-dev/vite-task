use core::alloc::{GlobalAlloc, Layout};

#[doc(hidden)]
pub struct ForbiddenGlobalAllocator;

unsafe extern "C" {
    // Do not return `!`: allocator calls carry optimizer attributes
    // that can erase calls to a known non-returning implementation.
    fn __fspy_global_allocator_must_not_be_used() -> *mut u8;
}

// SAFETY: no operation can execute: any retained call fails to link
// because the referenced symbol has no definition.
unsafe impl GlobalAlloc for ForbiddenGlobalAllocator {
    unsafe fn alloc(&self, _: Layout) -> *mut u8 {
        // SAFETY: this symbol must remain undefined; see above.
        unsafe { __fspy_global_allocator_must_not_be_used() }
    }

    unsafe fn dealloc(&self, _: *mut u8, _: Layout) {
        // SAFETY: this symbol must remain undefined; see above.
        unsafe { __fspy_global_allocator_must_not_be_used() };
    }

    unsafe fn alloc_zeroed(&self, _: Layout) -> *mut u8 {
        // SAFETY: this symbol must remain undefined; see above.
        unsafe { __fspy_global_allocator_must_not_be_used() }
    }

    unsafe fn realloc(&self, _: *mut u8, _: Layout, _: usize) -> *mut u8 {
        // SAFETY: this symbol must remain undefined; see above.
        unsafe { __fspy_global_allocator_must_not_be_used() }
    }
}

/// Registers a global allocator that fails to link if used.
///
/// Invoke once in the final binary as `fspy_nostd::no_global_alloc!()`.
/// This satisfies `alloc`'s allocator requirement while allowing collections
/// backed by explicit allocators. Every global allocation, deallocation,
/// zeroed allocation, or reallocation calls an intentionally undefined symbol.
///
/// The final linker must discard unused code and reject unresolved symbols.
/// This checks code retained after optimization, not every source-level call.
/// Shared libraries that allow unresolved symbols need stricter linker flags.
/// Do not define `__fspy_global_allocator_must_not_be_used` or register another
/// global allocator alongside this one.
#[macro_export]
macro_rules! no_global_alloc {
    () => {
        const _: () = {
            #[::core::prelude::v1::global_allocator]
            static ALLOCATOR: $crate::no_global_alloc::ForbiddenGlobalAllocator =
                $crate::no_global_alloc::ForbiddenGlobalAllocator;
        };
    };
}
