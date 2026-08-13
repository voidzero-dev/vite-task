//! Memory mappings with no process-runtime dependency.

#[cfg(unix)]
pub use rustix::mm::{MapFlags, MprotectFlags, ProtFlags, mmap, mmap_anonymous, mprotect, munmap};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;
