//! A closeable writer gate built from one shared word.
//!
//! Word layout: bit 63 is `CLOSED`, bits 0..63 count the guards currently held.
//! The word must read `0` before first use; callers get that from
//! zero-initialized memory.
//!
//! A writer takes a guard through [`Gate::enter`] and drops it when its work is
//! finished. The reader side calls [`Gate::close`] exactly once at the moment it
//! wants the memory frozen. `close` returns the number of guards that were still
//! held. Zero means every guard ever issued has already been dropped, and
//! everything written while those guards were held happens-before the `close`.
//!
//! # Why one word
//!
//! The closed bit and the count share a word on purpose. The near-counterexample
//! is: a writer reads the word, sees it open, the reader closes, and the writer
//! then increments and writes into memory the reader is already reading. That
//! cannot happen here because both halves are read-modify-write operations on
//! the same location, so they are totally ordered by that location's
//! modification order. Either the writer's compare-and-swap precedes the close,
//! and then the close's return value counts it, so the reader refuses to read;
//! or the close precedes it, and the writer's retry observes `CLOSED` and never
//! claims. A stale load never turns into a successful claim, because the
//! decision *is* the compare-and-swap. Splitting the bit and the count into two
//! atomics would reintroduce exactly that race.
//!
//! # Why `close() == 0` means frozen
//!
//! A guard is released only after the work it protects is complete, so a zero
//! count proves that every admitted claim ran to completion. The `Release` on
//! guard drop and the `Acquire` on `close` then make those writes visible to the
//! reader, not merely finished.
//!
//! A process killed between `enter` and the guard's drop leaks its count
//! permanently. That fails closed: `close` reports a nonzero count forever and
//! the caller never reads the memory.

use std::sync::atomic::{AtomicU64, Ordering};

/// Set once the gate is closed. No claim is admitted afterwards.
const CLOSED: u64 = 1 << 63;
/// The guard count occupies every bit below [`CLOSED`].
const COUNT_MASK: u64 = CLOSED - 1;

/// A gate over one caller-provided word.
pub(super) struct Gate<'a> {
    word: &'a AtomicU64,
}

/// Why [`Gate::enter`] refused to admit a writer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EnterError {
    /// The gate is closed. The writer must discard its work silently.
    Closed,
    /// The guard count saturated. Unreachable in practice; refusing is still
    /// better than wrapping into the closed bit.
    Saturated,
}

impl<'a> Gate<'a> {
    /// Wraps `word`, which must read `0` before the first [`Gate::enter`] or
    /// [`Gate::close`].
    pub(super) const fn new(word: &'a AtomicU64) -> Self {
        Self { word }
    }

    /// Admits a writer and counts it in one compare-and-swap.
    ///
    /// Checking the closed bit and publishing the increment are the same
    /// successful operation, so there is no observable "decided to enter but not
    /// yet counted" state.
    pub(super) fn enter(&self) -> Result<GateGuard<'a>, EnterError> {
        // Relaxed is enough. Entering publishes no data: the writer's stores
        // come after it on the same thread, and the read side learns about
        // them through the guard's release drop, not through this increment.
        // Atomic read-modify-writes always see the newest value of the word,
        // whatever their ordering, so the closed check cannot act on a stale
        // word either.
        self.word
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                if current & CLOSED != 0 || current & COUNT_MASK == COUNT_MASK {
                    None
                } else {
                    Some(current + 1)
                }
            })
            .map(|_| GateGuard { word: self.word })
            .map_err(
                |current| {
                    if current & CLOSED == 0 { EnterError::Saturated } else { EnterError::Closed }
                },
            )
    }

    /// Closes the gate and reports how many guards were still held.
    ///
    /// Idempotent: each call reports the count at that moment. A return value of
    /// `0` is the proof that the guarded memory is frozen and fully visible.
    ///
    /// Why acquire is enough even though enters are relaxed: a count of `0`
    /// means the word's history balances, so the operation right before this
    /// one in the word's modification order must be a guard's release drop.
    /// Read-modify-writes carry release sequences forward, so this acquire
    /// synchronizes with every earlier release drop, not just the newest one,
    /// and everything each writer stored before dropping its guard is visible
    /// from here on.
    pub(super) fn close(&self) -> u64 {
        self.word.fetch_or(CLOSED, Ordering::AcqRel) & COUNT_MASK
    }
}

/// A live writer's token. Dropping it makes everything written while it was
/// held visible to whoever closes the gate.
#[derive(Debug)]
pub(super) struct GateGuard<'a> {
    word: &'a AtomicU64,
}

impl Drop for GateGuard<'_> {
    fn drop(&mut self) {
        self.word.fetch_sub(1, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::atomic::AtomicUsize, thread};

    use super::*;

    #[test]
    fn enter_and_drop_balance_the_count() {
        let word = AtomicU64::new(0);
        let gate = Gate::new(&word);

        let first = gate.enter().unwrap();
        let second = gate.enter().unwrap();
        assert_eq!(word.load(Ordering::Relaxed) & COUNT_MASK, 2);

        drop(first);
        drop(second);
        assert_eq!(gate.close(), 0);
    }

    #[test]
    fn enter_is_refused_after_close() {
        let word = AtomicU64::new(0);
        let gate = Gate::new(&word);

        assert_eq!(gate.close(), 0);
        assert_eq!(gate.enter().unwrap_err(), EnterError::Closed);
        // Closing again is harmless and still reports no writer in flight.
        assert_eq!(gate.close(), 0);
    }

    #[test]
    fn close_reports_a_live_guard() {
        let word = AtomicU64::new(0);
        let gate = Gate::new(&word);

        let guard = gate.enter().unwrap();
        assert_eq!(gate.close(), 1);
        drop(guard);
        // The count drains even after the close, but the gate stays closed.
        assert_eq!(gate.close(), 0);
        assert_eq!(gate.enter().unwrap_err(), EnterError::Closed);
    }

    #[test]
    fn leaked_guard_keeps_the_count_forever() {
        let word = AtomicU64::new(0);
        let gate = Gate::new(&word);

        std::mem::forget(gate.enter().unwrap());

        assert_eq!(gate.close(), 1);
        assert_eq!(gate.close(), 1);
    }

    #[test]
    fn enter_refuses_a_saturated_count() {
        let word = AtomicU64::new(COUNT_MASK);
        let gate = Gate::new(&word);

        assert_eq!(gate.enter().unwrap_err(), EnterError::Saturated);
    }

    /// The invariant the whole protocol rests on: whenever `close` returns 0,
    /// every successful `enter`'s side effect is already visible, and every
    /// later `enter` fails.
    #[test]
    fn close_returning_zero_means_frozen() {
        const WRITERS: usize = 4;
        const ROUNDS: usize = 64;

        for _ in 0..ROUNDS {
            let word = AtomicU64::new(0);
            // Stands in for the shared memory the gate protects.
            let published = AtomicUsize::new(0);
            let admitted = AtomicUsize::new(0);

            let (in_flight, published_at_close) = thread::scope(|scope| {
                for _ in 0..WRITERS {
                    scope.spawn(|| {
                        let gate = Gate::new(&word);
                        if let Ok(guard) = gate.enter() {
                            admitted.fetch_add(1, Ordering::Relaxed);
                            // The side effect the guard protects.
                            published.fetch_add(1, Ordering::Relaxed);
                            drop(guard);
                        }
                    });
                }
                let in_flight = Gate::new(&word).close();
                (in_flight, published.load(Ordering::Relaxed))
            });

            if in_flight == 0 {
                // A writer can only be admitted before the close, so every
                // admitted writer had already dropped its guard, and the
                // close's acquire makes its effect visible at that instant.
                assert_eq!(
                    published_at_close,
                    admitted.load(Ordering::Relaxed),
                    "a writer was admitted but its effect was not published at close"
                );
            }
            // Whatever the close observed, the gate is shut for good.
            assert_eq!(Gate::new(&word).enter().unwrap_err(), EnterError::Closed);
        }
    }
}
