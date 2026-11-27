/// Reserved fixed-address region `(base, limit)` for the freestanding x86-64
/// launcher, inside LLVM's Linux `ASan` and C/C++ `TSan` 85–90 TiB application range.
pub const LAUNCHER_REGION_X86_64: (u64, u64) = (0x5500_0000_0000, 0x5a00_0000_0000);

/// Reserved fixed-address region `(base, limit)` for the freestanding `AArch64`
/// launcher, inside 39-bit virtual address space and `TSan`'s low 20 GiB range.
pub const LAUNCHER_REGION_AARCH64: (u64, u64) = (0x0002_0000_0000, 0x0005_0000_0000);
