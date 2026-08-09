//! Slab memory layout and the [`Slab`] view type.
//!
//! Each size class carves fixed-size blocks out of [`SLAB_SIZE`]-byte,
//! `SLAB_SIZE`-aligned slabs. A slab looks like this:
//!
//! ```text
//! | SlabHeader | links: [AtomicU32; blocks] | pad to 4 KiB | block 0 | block 1 | ... |
//! ```
//!
//! Free blocks are chained through the `links` side table — never through
//! block memory itself — so block payloads are only ever plain data. That
//! keeps every concurrent access well-defined under the memory model (no
//! mixed atomic/non-atomic reads of the same bytes) and Miri-clean.

use core::{
    num::NonZero,
    ptr::NonNull,
    sync::atomic::{AtomicU32, AtomicU64},
};

use crate::class::{CLASS_COUNT, MAX_POOL_ALIGN, block_size};

/// Slab size and alignment. Masking a block address with `!(SLAB_SIZE - 1)`
/// recovers its slab base.
pub const SLAB_SIZE: usize = 1 << 20;

const fn round_up(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}

/// Byte offset of the block area inside a slab holding `blocks` blocks.
const fn block_area_offset(blocks: usize) -> usize {
    round_up(size_of::<SlabHeader>() + blocks * size_of::<u32>(), MAX_POOL_ALIGN)
}

const fn compute_blocks_per_slab(class: usize) -> usize {
    let size = block_size(class);
    // Upper bound ignoring metadata, then shrink until header, link table,
    // padding, and blocks all fit in one slab.
    let mut blocks = SLAB_SIZE / size;
    while block_area_offset(blocks) + blocks * size > SLAB_SIZE {
        blocks -= 1;
    }
    blocks
}

/// Blocks per slab, for each size class.
pub const BLOCKS_PER_SLAB: [usize; CLASS_COUNT] = {
    let mut table = [0usize; CLASS_COUNT];
    let mut class = 0;
    while class < CLASS_COUNT {
        table[class] = compute_blocks_per_slab(class);
        class += 1;
    }
    table
};

const _: () = {
    assert!(size_of::<SlabHeader>() == 16);
    assert!(MAX_POOL_ALIGN >= align_of::<SlabHeader>());
    let mut class = 0;
    while class < CLASS_COUNT {
        assert!(BLOCKS_PER_SLAB[class] > 0);
        assert!(
            block_area_offset(BLOCKS_PER_SLAB[class]) + BLOCKS_PER_SLAB[class] * block_size(class)
                <= SLAB_SIZE
        );
        class += 1;
    }
};

/// Lives at the base of every slab mapping, ahead of the link table.
#[repr(C)]
struct SlabHeader {
    /// This slab's index in its class's slab table.
    slab_idx: u32,
    _reserved: u32,
    /// Number of blocks ever carved off this slab (monotonic; values at or
    /// beyond the class's blocks-per-slab mean the slab is exhausted). 64-bit
    /// so over-counting by racing threads can never wrap it around.
    carved: AtomicU64,
}

/// A view of one live slab mapping of a particular class.
///
/// # Invariant
///
/// `base` points to a [`SLAB_SIZE`]-byte, `SLAB_SIZE`-aligned mapping laid
/// out for `class` (initialized header, link table, block area) that is
/// never unmapped while the pool is in use. All raw-pointer arithmetic on
/// slab memory lives in this type's methods; the unsafe constructors are the
/// only places the invariant is asserted, and every accessor then relies on
/// it for bounds and liveness.
#[derive(Clone, Copy)]
pub struct Slab {
    base: NonNull<u8>,
    class: usize,
}

impl Slab {
    /// Wraps a slab pointer loaded from a class's slab table.
    ///
    /// # Safety
    ///
    /// `base` must be a non-null entry of the class's slab table. Such
    /// entries are only published after [`Slab::init_header`] ran on a
    /// suitable mapping, and are never cleared or unmapped while the pool is
    /// in use, so the type invariant holds.
    pub const unsafe fn from_published(base: NonNull<u8>, class: usize) -> Self {
        Self { base, class }
    }

    /// Recovers the slab containing a live pool block: slabs are
    /// `SLAB_SIZE`-aligned, so masking the block address's low bits yields
    /// the slab base (`with_addr` keeps the block pointer's provenance, which
    /// covers the whole slab mapping it was carved from). Returns `None` only
    /// for an address whose masked base would be null, which no real block
    /// can produce.
    ///
    /// # Safety
    ///
    /// `block` must be a block of `class` previously handed out by this pool
    /// and thus carved from a published slab of this class.
    pub unsafe fn of_block(block: NonNull<u8>, class: usize) -> Option<Self> {
        let base_addr = NonZero::new(block.as_ptr().addr() & !(SLAB_SIZE - 1))?;
        Some(Self { base: block.with_addr(base_addr), class })
    }

    /// Initializes the header of a fresh slab mapping, making it publishable
    /// into a slab table.
    ///
    /// # Safety
    ///
    /// `base` must point to a fresh, exclusive, zero-initialized,
    /// `SLAB_SIZE`-byte and `SLAB_SIZE`-aligned mapping. The zero fill
    /// doubles as the initial state of the carve counter and the link table.
    pub unsafe fn init_header(base: NonNull<u8>, slab_idx: u32) {
        // SAFETY: caller contract — the fresh exclusive mapping is aligned
        // (SLAB_SIZE-aligned, far beyond SlabHeader's needs) and large
        // enough for the header.
        unsafe {
            (*base.cast::<SlabHeader>().as_ptr()).slab_idx = slab_idx;
        }
    }

    const fn header(&self) -> &SlabHeader {
        // SAFETY: type invariant — the mapping is live, SLAB_SIZE-aligned
        // (far beyond SlabHeader's needs), and its header was initialized
        // before publication; the only non-atomic field is never written
        // again afterwards.
        unsafe { self.base.cast::<SlabHeader>().as_ref() }
    }

    /// This slab's index in its class's slab table.
    pub const fn slab_idx(&self) -> usize {
        self.header().slab_idx as usize
    }

    /// The monotonic carve counter.
    pub const fn carved(&self) -> &AtomicU64 {
        &self.header().carved
    }

    const fn blocks(&self) -> usize {
        BLOCKS_PER_SLAB[self.class]
    }

    /// The free-list link slot of `block_idx`.
    pub fn link(&self, block_idx: usize) -> &AtomicU32 {
        debug_assert!(block_idx < self.blocks());
        #[expect(
            clippy::cast_ptr_alignment,
            reason = "the link table starts at offset 16 of a SLAB_SIZE-aligned slab, so entries are 4-byte aligned"
        )]
        // SAFETY: type invariant plus the bound above put the slot within
        // the slab's link table; slots are only ever accessed atomically,
        // and the mapping outlives any borrow.
        unsafe {
            AtomicU32::from_ptr(
                self.base.as_ptr().add(size_of::<SlabHeader>()).cast::<u32>().add(block_idx),
            )
        }
    }

    /// The address of block `block_idx`.
    pub fn block(&self, block_idx: usize) -> NonNull<u8> {
        debug_assert!(block_idx < self.blocks());
        let offset = block_area_offset(self.blocks()) + block_idx * block_size(self.class);
        // SAFETY: type invariant plus the bound above keep `offset` within
        // the SLAB_SIZE mapping.
        unsafe { self.base.add(offset) }
    }

    /// The index of a block previously returned by [`Slab::block`].
    pub fn block_index(&self, block: NonNull<u8>) -> usize {
        let offset =
            block.as_ptr().addr() - self.base.as_ptr().addr() - block_area_offset(self.blocks());
        debug_assert_eq!(offset % block_size(self.class), 0);
        let block_idx = offset / block_size(self.class);
        debug_assert!(block_idx < self.blocks());
        block_idx
    }
}
