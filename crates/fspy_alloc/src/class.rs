//! Size-class policy: which layouts the pool serves, and at what block size.

use core::alloc::Layout;

const MIN_CLASS_SHIFT: u32 = 4;
const MAX_CLASS_SHIFT: u32 = 16;
pub const CLASS_COUNT: usize = (MAX_CLASS_SHIFT - MIN_CLASS_SHIFT) as usize + 1;
const MIN_BLOCK_SIZE: usize = 1 << MIN_CLASS_SHIFT;
pub const MAX_BLOCK_SIZE: usize = 1 << MAX_CLASS_SHIFT;
/// Block areas start at this alignment within a slab, making it the largest
/// alignment the pool can serve; stricter layouts map directly.
pub const MAX_POOL_ALIGN: usize = 4096;

pub const fn block_size(class: usize) -> usize {
    1 << (MIN_CLASS_SHIFT as usize + class)
}

/// Returns the size class for `layout`, or `None` if the request must be
/// mapped directly (too large or over-aligned).
pub const fn class_of(layout: Layout) -> Option<usize> {
    if layout.align() > MAX_POOL_ALIGN {
        return None;
    }
    let mut size = layout.size();
    // A block of `size >= align` at a `min(block size, 4 KiB)` boundary is
    // aligned to `align` (both are powers of two and `align <= 4 KiB`).
    if size < layout.align() {
        size = layout.align();
    }
    if size < MIN_BLOCK_SIZE {
        size = MIN_BLOCK_SIZE;
    }
    if size > MAX_BLOCK_SIZE {
        return None;
    }
    let shift = size.next_power_of_two().trailing_zeros();
    Some((shift - MIN_CLASS_SHIFT) as usize)
}
