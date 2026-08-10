// Shared memory size for storing path accesses.
// 4 GiB is large enough to store path accesses in almost any realistic scenario.
// This doesn't allocate physical memory until it's actually used.
pub const SHM_CAPACITY: usize = 4 * 1024 * 1024 * 1024;
