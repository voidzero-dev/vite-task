//! Anonymous memory mappings.
//!
//! Re-exposed from rustix as-is: these are single syscalls against the
//! kernel's own address-space bookkeeping — no libc state, no locks, no
//! allocation — so they already meet this crate's rules everywhere it
//! promises to work. What this module adds is the curation (being listed
//! here is what marks them safe for signal handlers, fork children, and
//! pre-libc startup) and the crate-level backend check, which guarantees
//! they cannot silently turn into libc calls on Linux.

pub use rustix::mm::{MapFlags, MprotectFlags, ProtFlags, mmap, mmap_anonymous, mprotect, munmap};
