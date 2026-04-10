#[cfg(target_os = "android")]
mod memfd;

#[cfg(not(target_os = "android"))]
mod shmem;

#[cfg(target_os = "android")]
pub use memfd::*;
#[cfg(not(target_os = "android"))]
pub use shmem::*;
