mod create_process;
mod find_file;
mod nt;

use constcat::concat_slices;

use super::detour::DetourAny;

pub const DETOURS: &[DetourAny] = concat_slices!([DetourAny]:
    create_process::DETOURS,
    find_file::DETOURS,
    nt::DETOURS,
);
