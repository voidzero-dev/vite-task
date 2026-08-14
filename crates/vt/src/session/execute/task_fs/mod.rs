//! Filesystem fingerprinting for task caching.
//!
//! One run of a task, from the filesystem's point of view:
//!
//! - [`TaskFs::pre_run`] — before the task executes: capture the state of its
//!   configured inputs and detect what changed since a previous run.
//! - [`TaskFs::post_run`] — after it finished: judge the traced file accesses
//!   and produce everything the cache should remember ([`Conclusion`]).
//!
//! [`InputFingerprints`] is the opaque record that links the two: a run's
//! `post_run` produces it, the caller stores it, and a later run's `pre_run`
//! checks the filesystem against it.

mod collections;
mod fingerprint;
mod glob;
mod hash;
mod task_run;
mod tracked_accesses;

pub use fingerprint::{InputChange, InputChangeKind, InputFingerprints};
pub use task_run::{Conclusion, PostRunError, TaskFs};
