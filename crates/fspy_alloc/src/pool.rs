//! Lock-free size-class pool.
//!
//! Layouts round up to a power-of-two class (see the `class` module) whose
//! blocks are carved from slabs (see the `slab` module for the memory
//! layout).
//!
//! # Concurrency
//!
//! Each class has a Treiber-stack free list plus a bump cursor over the
//! newest ("active") slab:
//!
//! - **alloc** pops the free list, or carves the next block off the active
//!   slab, installing a fresh slab when the active one is exhausted.
//! - **dealloc** pushes the block back onto its class's free list.
//!
//! The 64-bit list head packs a 40-bit generation tag next to the 24-bit
//! block reference; the tag advances on every successful push and pop, which
//! makes the classic Treiber-stack ABA failure require a thread to stall
//! between its head load and CAS while other threads perform an exact
//! multiple of 2^40 head mutations. That is a probabilistic defense, not a
//! formal impossibility — see [`HEAD_TAG_SHIFT`]. Slab installation races
//! are resolved with a compare-and-swap on the slab table slot; the loser
//! simply returns its (never-published) mapping.
//!
//! The pool is **lock-free, not wait-free**: a CAS loop can retry
//! indefinitely under contention, but a retry only ever happens because
//! another thread completed an operation, so system-wide progress never
//! stalls — and, the property fork- and signal-safety actually require, no
//! operation ever waits on state that only a suspended or vanished thread
//! could release. Blocks are aligned to `min(block size, 4 KiB)`; requests
//! with stricter alignment (or size beyond the largest class) bypass the
//! pool and map directly.

use core::{
    alloc::Layout,
    marker::PhantomData,
    ptr,
    ptr::NonNull,
    sync::atomic::{AtomicPtr, AtomicU32, AtomicU64, Ordering},
};

use crate::{
    class::{CLASS_COUNT, class_of},
    mapping::Mapping,
    slab::{BLOCKS_PER_SLAB, SLAB_SIZE, Slab},
    sys::Sys,
};

/// Bounded by the 8 bits reserved for slab indices in a packed block
/// reference. Caps each class at ~200 MiB.
const MAX_SLABS_PER_CLASS: usize = 256;

/// Sentinel for "no block" in the 24-bit packed-reference field of a
/// free-list head. Never collides with a real reference: block indices stay
/// below `0xFFFF` (asserted with the slab geometry below).
const NO_BLOCK: u32 = 0x00FF_FFFF;
/// Sentinel for "no slab installed yet" in [`ClassState::active`].
const NO_SLAB: u32 = u32::MAX;

const _: () = {
    assert!(MAX_SLABS_PER_CLASS <= 1 << 8, "slab index must fit in 8 bits");
    let mut class = 0;
    while class < CLASS_COUNT {
        assert!(
            BLOCKS_PER_SLAB[class] <= 0xFFFF,
            "block indices must fit in 16 bits and stay below the NO_BLOCK sentinel"
        );
        class += 1;
    }
};

/// Packs a block's location into 24 bits: slab index in bits 16..24, block
/// index in bits 0..16.
#[expect(
    clippy::cast_possible_truncation,
    reason = "callers pass indices bounded by MAX_SLABS_PER_CLASS and BLOCKS_PER_SLAB"
)]
const fn pack_ref(slab_idx: usize, block_idx: usize) -> u32 {
    ((slab_idx as u32) << 16) | (block_idx as u32)
}

const fn unpack_ref(packed: u32) -> (usize, usize) {
    (((packed >> 16) & 0xFF) as usize, (packed & 0xFFFF) as usize)
}

/// A free-list head is `[generation tag : 40 | packed block reference : 24]`.
/// The tag advances on every successful push and pop, so a stale
/// compare-and-swap can only succeed if its thread stalls between head load
/// and CAS while others perform an exact multiple of 2^40 head mutations —
/// not a formal impossibility, but hours of maximum-rate churn inside one
/// stalled instruction window.
const HEAD_TAG_SHIFT: u32 = 24;
const HEAD_TAG_MASK: u64 = (1 << 40) - 1;
const HEAD_REF_MASK: u64 = (1 << HEAD_TAG_SHIFT) - 1;

/// Splits a free-list head into `(generation tag, packed block reference)`.
const fn head_parts(head: u64) -> (u64, u32) {
    // The mask keeps the reference within 24 bits, so the cast is lossless.
    (head >> HEAD_TAG_SHIFT, (head & HEAD_REF_MASK) as u32)
}

const fn head_from_parts(tag: u64, block_ref: u32) -> u64 {
    ((tag & HEAD_TAG_MASK) << HEAD_TAG_SHIFT) | block_ref as u64
}

/// Aligned to its own cache-line region so hot heads of different classes
/// don't false-share.
#[repr(align(128))]
struct ClassState {
    /// Treiber free-list head: `[generation tag : 40 | packed ref : 24]`.
    head: AtomicU64,
    /// Index of the slab currently being carved, or [`NO_SLAB`] before the
    /// first slab is installed.
    active: AtomicU32,
    /// Base addresses of installed slabs. Written once (null → mapping) and
    /// never cleared.
    slabs: [AtomicPtr<u8>; MAX_SLABS_PER_CLASS],
}

impl ClassState {
    const fn new() -> Self {
        Self {
            head: AtomicU64::new(head_from_parts(0, NO_BLOCK)),
            active: AtomicU32::new(NO_SLAB),
            slabs: [const { AtomicPtr::new(ptr::null_mut()) }; MAX_SLABS_PER_CLASS],
        }
    }
}

/// A block handed out by [`Pool::alloc_block`], remembering whether it came
/// off the free list (may contain stale data) or was carved fresh off a slab
/// (still kernel-zeroed).
enum ClassBlock {
    Recycled(NonNull<u8>),
    Carved(NonNull<u8>),
}

enum Carve {
    Block(NonNull<u8>),
    /// A new slab was (or concurrently got) installed; retry the allocation.
    Retry,
    /// Out of slab table entries or out of memory.
    Exhausted,
}

/// The allocator core, generic over its [`Sys`] memory provider.
pub struct Pool<S: Sys> {
    classes: [ClassState; CLASS_COUNT],
    sys: PhantomData<fn() -> S>,
}

impl<S: Sys> Pool<S> {
    #[expect(
        clippy::large_stack_arrays,
        reason = "the class table (~30 KiB) is only ever materialized into a const-initialized static, never built on a runtime stack"
    )]
    pub const fn new() -> Self {
        Self { classes: [const { ClassState::new() }; CLASS_COUNT], sys: PhantomData }
    }

    /// Allocates memory for `layout`. Returns `None` on exhaustion.
    pub fn alloc(&self, layout: Layout) -> Option<NonNull<u8>> {
        match class_of(layout) {
            Some(class) => match self.alloc_block(class)? {
                ClassBlock::Recycled(ptr) | ClassBlock::Carved(ptr) => Some(ptr),
            },
            // Deliberately leaked until `dealloc` reclaims ownership.
            None => Some(Mapping::<S>::new(layout.size(), layout.align())?.into_raw()),
        }
    }

    /// Like [`Pool::alloc`], but the returned memory is zeroed.
    pub fn alloc_zeroed(&self, layout: Layout) -> Option<NonNull<u8>> {
        match class_of(layout) {
            Some(class) => match self.alloc_block(class)? {
                // Freshly carved blocks are still kernel-zeroed.
                ClassBlock::Carved(ptr) => Some(ptr),
                ClassBlock::Recycled(ptr) => {
                    // SAFETY: the block is freshly allocated, exclusively
                    // ours, and at least `layout.size()` bytes.
                    unsafe { ptr.as_ptr().write_bytes(0, layout.size()) };
                    Some(ptr)
                }
            },
            // Fresh mappings are zeroed by the kernel; deliberately leaked
            // until `dealloc` reclaims ownership.
            None => Some(Mapping::<S>::new(layout.size(), layout.align())?.into_raw()),
        }
    }

    /// Releases memory obtained from this pool.
    ///
    /// # Safety
    ///
    /// `ptr` must have been returned by this pool for exactly this `layout`
    /// and must not be used afterwards.
    pub unsafe fn dealloc(&self, ptr: NonNull<u8>, layout: Layout) {
        match class_of(layout) {
            // SAFETY: caller contract — a `Some(class)` layout was served
            // from the pool, so `ptr` is a live block of this class.
            Some(class) => unsafe { self.push_free(ptr, class) },
            // SAFETY: caller contract — a `None` layout was served by the
            // mapping path with these parameters and is no longer in use, so
            // ownership can be reclaimed (and the region dropped).
            None => drop(unsafe { Mapping::<S>::from_raw(ptr, layout.size(), layout.align()) }),
        }
    }

    /// Grows or shrinks an allocation, preserving contents up to the smaller
    /// of the old and new sizes. Returns `None` on exhaustion (the original
    /// allocation stays valid).
    ///
    /// # Safety
    ///
    /// `ptr` must have been returned by this pool for exactly `layout`, and
    /// `new_size` must be non-zero.
    pub unsafe fn realloc(
        &self,
        ptr: NonNull<u8>,
        layout: Layout,
        new_size: usize,
    ) -> Option<NonNull<u8>> {
        let new_layout = Layout::from_size_align(new_size, layout.align()).ok()?;
        let class = class_of(layout);
        if class.is_some() && class == class_of(new_layout) {
            // Same size class: the existing block already fits.
            return Some(ptr);
        }
        let new_ptr = self.alloc(new_layout)?;
        // SAFETY: `new_ptr` is a fresh exclusive allocation of at least
        // `new_size` bytes; `ptr` is valid for `layout.size()` bytes (caller
        // contract); distinct allocations never overlap.
        unsafe {
            new_ptr.as_ptr().copy_from_nonoverlapping(ptr.as_ptr(), layout.size().min(new_size));
        }
        // SAFETY: caller contract — `ptr` came from this pool with `layout`.
        unsafe { self.dealloc(ptr, layout) };
        Some(new_ptr)
    }

    fn alloc_block(&self, class: usize) -> Option<ClassBlock> {
        loop {
            if let Some(ptr) = self.pop_free(class) {
                return Some(ClassBlock::Recycled(ptr));
            }
            match self.carve(class) {
                Carve::Block(ptr) => return Some(ClassBlock::Carved(ptr)),
                Carve::Retry => {}
                Carve::Exhausted => return None,
            }
        }
    }

    /// Returns the published slab of `class` at `slab_idx`, if installed.
    fn published_slab(&self, class: usize, slab_idx: usize) -> Option<Slab> {
        let base = NonNull::new(self.classes[class].slabs[slab_idx].load(Ordering::Acquire))?;
        // SAFETY: non-null slab-table entries are only published (with
        // `Release`, paired with the `Acquire` above) after
        // `Slab::init_header` ran on a fresh SLAB_SIZE-aligned mapping, and
        // are never cleared or unmapped while the pool is in use.
        Some(unsafe { Slab::from_published(base, class) })
    }

    /// Pops a block off the class's free list.
    fn pop_free(&self, class: usize) -> Option<NonNull<u8>> {
        let state = &self.classes[class];
        loop {
            let head = state.head.load(Ordering::Acquire);
            let (tag, block_ref) = head_parts(head);
            if block_ref == NO_BLOCK {
                return None;
            }
            let (slab_idx, block_idx) = unpack_ref(block_ref);
            // A listed block was carved from its slab, so the slab is always
            // published; `None` here is unreachable in practice.
            let slab = self.published_slab(class, slab_idx)?;
            // Read the successor link before the CAS. If another thread pops
            // this block first, the tag comparison below fails and the value
            // read here is discarded; since links live in the atomic side
            // table, the racing read itself is well-defined.
            let next = slab.link(block_idx).load(Ordering::Relaxed);
            let new_head = head_from_parts(tag.wrapping_add(1), next);
            if state
                .head
                .compare_exchange_weak(head, new_head, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                return Some(slab.block(block_idx));
            }
        }
    }

    /// Pushes a block onto the class's free list.
    ///
    /// # Safety
    ///
    /// `ptr` must be a block of `class` previously returned by this pool and
    /// no longer in use.
    unsafe fn push_free(&self, ptr: NonNull<u8>, class: usize) {
        // SAFETY: caller contract — `ptr` is a live block of `class` from
        // this pool. (The impossible `None` would merely leak the block.)
        let Some(slab) = (unsafe { Slab::of_block(ptr, class) }) else { return };
        let block_idx = slab.block_index(ptr);
        let packed = pack_ref(slab.slab_idx(), block_idx);
        let link = slab.link(block_idx);
        let state = &self.classes[class];
        loop {
            let head = state.head.load(Ordering::Relaxed);
            let (tag, head_ref) = head_parts(head);
            link.store(head_ref, Ordering::Relaxed);
            let new_head = head_from_parts(tag.wrapping_add(1), packed);
            // `Release` publishes the link store above to the eventual popper.
            if state
                .head
                .compare_exchange_weak(head, new_head, Ordering::Release, Ordering::Relaxed)
                .is_ok()
            {
                return;
            }
        }
    }

    /// Carves the next block off the class's active slab, installing a new
    /// slab if the active one is exhausted (or none exists yet).
    fn carve(&self, class: usize) -> Carve {
        let state = &self.classes[class];
        let active = state.active.load(Ordering::Acquire);
        // `active` is only published after its slab pointer, so a live
        // `active` always resolves to a published slab.
        if active != NO_SLAB
            && let Some(slab) = self.published_slab(class, active as usize)
        {
            let carved_idx = slab.carved().fetch_add(1, Ordering::Relaxed);
            if let Some(block_idx) = carved_to_block_idx(carved_idx, class) {
                return Carve::Block(slab.block(block_idx));
            }
            // Active slab exhausted; fall through to install the next one.
        }
        let next_idx = if active == NO_SLAB { 0 } else { active as usize + 1 };
        if next_idx >= MAX_SLABS_PER_CLASS {
            return Carve::Exhausted;
        }
        self.install_slab(class, next_idx);
        if self.published_slab(class, next_idx).is_none() {
            // Our mapping failed and no other thread succeeded either.
            return Carve::Exhausted;
        }
        #[expect(clippy::cast_possible_truncation, reason = "bounded by MAX_SLABS_PER_CLASS")]
        let next_active = next_idx as u32;
        // Advance `active`; losing the race just means another thread already
        // advanced it. Either way the retry re-reads it.
        let _ =
            state.active.compare_exchange(active, next_active, Ordering::AcqRel, Ordering::Relaxed);
        Carve::Retry
    }

    /// Maps and publishes the slab at `slab_idx`, unless another thread beats
    /// us to it (or the mapping fails, leaving the slot null).
    fn install_slab(&self, class: usize, slab_idx: usize) {
        let state = &self.classes[class];
        if !state.slabs[slab_idx].load(Ordering::Acquire).is_null() {
            return;
        }
        let Some(mapping) = Mapping::<S>::new(SLAB_SIZE, SLAB_SIZE) else { return };
        #[expect(clippy::cast_possible_truncation, reason = "bounded by MAX_SLABS_PER_CLASS")]
        let idx = slab_idx as u32;
        // SAFETY: `mapping` is a fresh, exclusive, zero-initialized,
        // SLAB_SIZE-byte and SLAB_SIZE-aligned mapping.
        unsafe { Slab::init_header(mapping.ptr(), idx) };
        if state.slabs[slab_idx]
            .compare_exchange(
                ptr::null_mut(),
                mapping.ptr().as_ptr(),
                Ordering::Release,
                Ordering::Relaxed,
            )
            .is_ok()
        {
            // Published: the slab now lives for the rest of the process.
            let _ = mapping.into_raw();
        }
        // Otherwise another thread installed this slot first; our
        // never-published mapping is released when `mapping` drops.
    }

    /// Tears down all slab mappings. Test-only: the global allocator lives in
    /// a static and never releases its slabs, but tests (and Miri's leak
    /// checker) want a clean shutdown.
    #[cfg(test)]
    fn unmap_all_slabs(&mut self) {
        for state in &mut self.classes {
            *state.head.get_mut() = head_from_parts(0, NO_BLOCK);
            *state.active.get_mut() = NO_SLAB;
            for slot in &mut state.slabs {
                let slab = core::mem::replace(slot.get_mut(), ptr::null_mut());
                if let Some(base) = NonNull::new(slab) {
                    // SAFETY: `base` was leaked into the table by
                    // `install_slab` with these parameters; `&mut self`
                    // guarantees no concurrent (or future) use of its blocks.
                    drop(unsafe { Mapping::<S>::from_raw(base, SLAB_SIZE, SLAB_SIZE) });
                }
            }
        }
    }
}

/// Converts a raw carve-counter value into a block index, or `None` if the
/// slab is exhausted.
fn carved_to_block_idx(carved: u64, class: usize) -> Option<usize> {
    let idx = usize::try_from(carved).ok()?;
    (idx < BLOCKS_PER_SLAB[class]).then_some(idx)
}

#[cfg(test)]
mod tests {
    use std::{boxed::Box, sync::mpsc, thread, vec::Vec};

    use super::*;
    use crate::class::{MAX_BLOCK_SIZE, MAX_POOL_ALIGN, block_size};

    /// Host-allocator-backed [`Sys`] so the pool core runs under Miri and on
    /// any platform.
    struct TestSys;

    impl Sys for TestSys {
        fn map(size: usize, align: usize) -> Option<NonNull<u8>> {
            let layout = Layout::from_size_align(size.max(1), align).ok()?;
            // SAFETY: `layout` has non-zero size.
            NonNull::new(unsafe { std::alloc::alloc_zeroed(layout) })
        }

        unsafe fn unmap(ptr: NonNull<u8>, size: usize, align: usize) {
            let layout = Layout::from_size_align(size.max(1), align).unwrap();
            // SAFETY: caller contract — `ptr` was returned by `map`, which
            // used exactly this layout.
            unsafe { std::alloc::dealloc(ptr.as_ptr(), layout) }
        }
    }

    fn with_pool(test: impl FnOnce(&Pool<TestSys>)) {
        let mut pool = Box::new(Pool::<TestSys>::new());
        test(&pool);
        pool.unmap_all_slabs();
    }

    fn layout(size: usize, align: usize) -> Layout {
        Layout::from_size_align(size, align).unwrap()
    }

    #[test]
    fn head_tag_wraps_within_its_field() {
        let head = head_from_parts(HEAD_TAG_MASK, 42);
        assert_eq!(head_parts(head), (HEAD_TAG_MASK, 42));
        // Advancing the maximal tag must wrap to zero without touching the
        // reference bits.
        let wrapped = head_from_parts(HEAD_TAG_MASK.wrapping_add(1), 42);
        assert_eq!(head_parts(wrapped), (0, 42));
    }

    #[test]
    fn packed_refs_cannot_collide_with_the_sentinel() {
        for &blocks in &BLOCKS_PER_SLAB {
            let max_ref = pack_ref(MAX_SLABS_PER_CLASS - 1, blocks - 1);
            assert_ne!(max_ref, NO_BLOCK);
            assert!(u64::from(max_ref) <= HEAD_REF_MASK);
        }
    }

    #[test]
    fn classify_boundaries() {
        assert_eq!(class_of(layout(1, 1)), Some(0));
        assert_eq!(class_of(layout(16, 1)), Some(0));
        assert_eq!(class_of(layout(17, 1)), Some(1));
        assert_eq!(class_of(layout(MAX_BLOCK_SIZE, 8)), Some(CLASS_COUNT - 1));
        assert_eq!(class_of(layout(MAX_BLOCK_SIZE + 1, 8)), None);
        // Alignment can raise the class.
        assert_eq!(class_of(layout(8, 1024)), class_of(layout(1024, 8)));
        // Over-aligned layouts bypass the pool.
        assert_eq!(class_of(layout(16, MAX_POOL_ALIGN * 2)), None);
    }

    #[test]
    fn round_trip_every_class() {
        with_pool(|pool| {
            for class in 0..CLASS_COUNT {
                let l = layout(block_size(class), 8);
                let ptr = pool.alloc(l).unwrap();
                // SAFETY: fresh exclusive allocation of `block_size` bytes.
                unsafe { ptr.as_ptr().write_bytes(0xAB, l.size()) };
                // SAFETY: reading back the block we just wrote.
                let last = unsafe { ptr.as_ptr().add(l.size() - 1).read() };
                assert_eq!(last, 0xAB);
                // SAFETY: allocated above with the same layout.
                unsafe { pool.dealloc(ptr, l) };
            }
        });
    }

    #[test]
    fn block_alignment() {
        with_pool(|pool| {
            for align in [8, 64, 1024, MAX_POOL_ALIGN] {
                let l = layout(24, align);
                let ptr = pool.alloc(l).unwrap();
                assert_eq!(ptr.as_ptr().addr() % align, 0, "align {align}");
                // SAFETY: allocated above with the same layout.
                unsafe { pool.dealloc(ptr, l) };
            }
        });
    }

    #[test]
    fn free_list_recycles_lifo() {
        with_pool(|pool| {
            let l = layout(100, 8);
            let first = pool.alloc(l).unwrap();
            // SAFETY: allocated above with the same layout.
            unsafe { pool.dealloc(first, l) };
            let second = pool.alloc(l).unwrap();
            assert_eq!(first, second);
            // SAFETY: allocated above with the same layout.
            unsafe { pool.dealloc(second, l) };
        });
    }

    #[test]
    fn carves_across_multiple_slabs() {
        with_pool(|pool| {
            // The largest class has the fewest blocks per slab, so a couple
            // dozen live allocations force several slab installations.
            let l = layout(MAX_BLOCK_SIZE, 8);
            let count = BLOCKS_PER_SLAB[CLASS_COUNT - 1] * 3 + 1;
            let blocks: Vec<NonNull<u8>> = (0..count).map(|_| pool.alloc(l).unwrap()).collect();
            for (i, ptr) in blocks.iter().enumerate() {
                assert!(blocks[..i].iter().all(|other| other != ptr), "duplicate block");
                // SAFETY: live exclusive allocation of `MAX_BLOCK_SIZE` bytes.
                unsafe { ptr.as_ptr().write_bytes(0x5A, l.size()) };
            }
            for ptr in blocks {
                // SAFETY: allocated above with the same layout.
                unsafe { pool.dealloc(ptr, l) };
            }
        });
    }

    #[test]
    fn large_allocations_bypass_pool() {
        with_pool(|pool| {
            for l in [layout(MAX_BLOCK_SIZE + 1, 8), layout(5 << 20, 8), layout(64, 8192)] {
                let ptr = pool.alloc(l).unwrap();
                assert_eq!(ptr.as_ptr().addr() % l.align(), 0);
                // SAFETY: fresh exclusive allocation of `l.size()` bytes.
                unsafe { ptr.as_ptr().write_bytes(0xCD, l.size()) };
                // SAFETY: allocated above with the same layout.
                unsafe { pool.dealloc(ptr, l) };
            }
        });
    }

    #[test]
    fn realloc_within_class_keeps_block() {
        with_pool(|pool| {
            let l = layout(100, 8);
            let ptr = pool.alloc(l).unwrap();
            // SAFETY: realloc contract — `ptr` allocated with `l`.
            let grown = unsafe { pool.realloc(ptr, l, 120) }.unwrap();
            assert_eq!(ptr, grown, "same class must realloc in place");
            // SAFETY: allocated above; 120 rounds to the same class as 100.
            unsafe { pool.dealloc(grown, layout(120, 8)) };
        });
    }

    #[test]
    fn realloc_across_classes_preserves_contents() {
        with_pool(|pool| {
            let l = layout(64, 8);
            let ptr = pool.alloc(l).unwrap();
            for i in 0..64u8 {
                // SAFETY: live exclusive allocation of 64 bytes.
                unsafe { ptr.as_ptr().add(usize::from(i)).write(i) };
            }
            // SAFETY: realloc contract — `ptr` allocated with `l`.
            let grown = unsafe { pool.realloc(ptr, l, 4096) }.unwrap();
            for i in 0..64u8 {
                // SAFETY: `grown` is live for 4096 bytes.
                let got = unsafe { grown.as_ptr().add(usize::from(i)).read() };
                assert_eq!(got, i);
            }
            // Shrink across classes, and from the large path back into a class.
            // SAFETY: `grown` allocated with the 4096 layout above.
            let shrunk = unsafe { pool.realloc(grown, layout(4096, 8), 8) }.unwrap();
            // SAFETY: `shrunk` is live for 8 bytes.
            assert_eq!(unsafe { shrunk.as_ptr().read() }, 0);
            // SAFETY: allocated above with the same layout.
            unsafe { pool.dealloc(shrunk, layout(8, 8)) };
        });
    }

    #[test]
    fn alloc_zeroed_scrubs_recycled_blocks() {
        with_pool(|pool| {
            let l = layout(256, 8);
            let dirty = pool.alloc(l).unwrap();
            // SAFETY: live exclusive allocation of 256 bytes.
            unsafe { dirty.as_ptr().write_bytes(0xFF, l.size()) };
            // SAFETY: allocated above with the same layout.
            unsafe { pool.dealloc(dirty, l) };

            let zeroed = pool.alloc_zeroed(l).unwrap();
            assert_eq!(
                zeroed, dirty,
                "must recycle the dirty block for this test to be meaningful"
            );
            for i in 0..l.size() {
                // SAFETY: live exclusive allocation of 256 bytes.
                assert_eq!(unsafe { zeroed.as_ptr().add(i).read() }, 0);
            }
            // SAFETY: allocated above with the same layout.
            unsafe { pool.dealloc(zeroed, l) };
        });
    }

    /// Cheap deterministic PRNG so the stress test needs no dependencies.
    fn lcg(state: &mut u64) -> u64 {
        *state =
            state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        *state >> 33
    }

    /// An owned allocation in flight between threads.
    struct SendBlock(NonNull<u8>, Layout);
    // SAFETY: SendBlock represents exclusive ownership of the block, which is
    // transferred wholesale to the receiving thread.
    unsafe impl Send for SendBlock {}

    #[test]
    fn concurrent_stress_with_cross_thread_frees() {
        const THREADS: usize = if cfg!(miri) { 3 } else { 8 };
        const OPS: usize = if cfg!(miri) { 60 } else { 20_000 };

        with_pool(|pool| {
            thread::scope(|scope| {
                // Each thread frees blocks allocated by its neighbor, so
                // pushes and pops of the same list run on different threads.
                // Bounded channels keep the number of in-flight blocks well
                // under the pool's per-class capacity regardless of thread
                // scheduling.
                let (senders, receivers): (Vec<_>, Vec<_>) =
                    (0..THREADS).map(|_| mpsc::sync_channel::<SendBlock>(64)).unzip();
                let mut senders_rotated: Vec<_> = senders.into_iter().map(Some).collect();
                senders_rotated.rotate_left(1);

                for (thread_idx, (receiver, sender)) in
                    receivers.into_iter().zip(&mut senders_rotated).enumerate()
                {
                    let sender = sender.take().unwrap();
                    let pool = &*pool;
                    scope.spawn(move || {
                        let mut rng = 0x9E37_79B9_7F4A_7C15_u64 ^ thread_idx as u64;
                        for _ in 0..OPS {
                            let size = 1 + usize::try_from(lcg(&mut rng)).unwrap()
                                % (3 * MAX_BLOCK_SIZE / 2);
                            let align = 1 << (lcg(&mut rng) % 7);
                            let l = layout(size, align);
                            let ptr = pool.alloc(l).unwrap();
                            let marker = u8::try_from(lcg(&mut rng) & 0xFF).unwrap();
                            // SAFETY: fresh exclusive allocation of `size` bytes.
                            unsafe {
                                ptr.as_ptr().write(marker);
                                ptr.as_ptr().add(size - 1).write(marker);
                            }
                            // SAFETY: we still exclusively own the block.
                            let (first, last) =
                                unsafe { (ptr.as_ptr().read(), ptr.as_ptr().add(size - 1).read()) };
                            assert_eq!((first, last), (marker, marker), "block corrupted");
                            if let Err(returned) = sender.try_send(SendBlock(ptr, l)) {
                                // Neighbor's queue is full (or it finished);
                                // free locally instead of blocking, which in
                                // a ring of senders could deadlock.
                                let (mpsc::TrySendError::Full(SendBlock(ptr, l))
                                | mpsc::TrySendError::Disconnected(SendBlock(ptr, l))) = returned;
                                // SAFETY: allocated above with layout `l`.
                                unsafe { pool.dealloc(ptr, l) };
                            }
                            // Drain what our own producer has sent so far,
                            // interleaving cross-thread pops with pushes.
                            while let Ok(SendBlock(ptr, l)) = receiver.try_recv() {
                                // SAFETY: the neighbor allocated `ptr` with
                                // `l` and transferred ownership over the
                                // channel.
                                unsafe { pool.dealloc(ptr, l) };
                            }
                        }
                        drop(sender);
                        for SendBlock(ptr, l) in receiver {
                            // SAFETY: the neighbor allocated `ptr` with `l`
                            // and transferred ownership over the channel.
                            unsafe { pool.dealloc(ptr, l) };
                        }
                    });
                }
            });
        });
    }
}
