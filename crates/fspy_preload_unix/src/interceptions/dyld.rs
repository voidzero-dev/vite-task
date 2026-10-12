//! dyld lists `DYLD_INSERT_LIBRARIES` images before the main executable, so
//! injecting the preload moves the main executable away from image index 0.
//! Code that assumes index 0 is the main executable then reads another
//! image's header or slide: Node.js single executable applications (via
//! postject's `getsectdata(..) + _dyld_get_image_vmaddr_slide(0)`) and
//! `deno compile` outputs fail to find their embedded payload. These
//! interposers present the main executable at index 0, followed by the
//! inserted libraries.
//!
//! dyld moves inserted libraries ahead of the main executable "for correct
//! flat namespace lookups" ([`RuntimeState::loadInsertedLibraries`]) and
//! applies the same permutation itself, but only for `x86_64` apps targeting
//! macOS before 12 ([`normalizeImageIndex`]). Where dyld already applies it,
//! the main executable is at index 0 and the mapping below is the identity.
//!
//! [`RuntimeState::loadInsertedLibraries`]: https://github.com/apple-oss-distributions/dyld/blob/fd8d0c4d52320ebf64db34f3cb280310d905c5ae/dyld/DyldRuntimeState.cpp#L676-L680
//! [`normalizeImageIndex`]: https://github.com/apple-oss-distributions/dyld/blob/fd8d0c4d52320ebf64db34f3cb280310d905c5ae/dyld/DyldAPIs.cpp#L238-L253

use crate::{
    libc::{c_char, intptr_t, mach_header},
    macros::intercept,
};

/// `MH_EXECUTE` from `<mach-o/loader.h>`.
const MH_EXECUTE: u32 = 0x2;

/// Maps an index in the presented order to dyld's actual index.
///
/// The main executable is looked up on every call rather than cached: its
/// index stays fixed only because of undocumented dyld behavior.
fn actual_image_index(image_index: u32) -> u32 {
    let mut main_index = 0;
    loop {
        // SAFETY: dyld returns null for out-of-range indices.
        let header = unsafe { _dyld_get_image_header::original()(main_index) };
        if header.is_null() {
            return image_index;
        }
        // SAFETY: a non-null header points to the loaded image's mach header.
        if unsafe { (*header).filetype } == MH_EXECUTE {
            break;
        }
        main_index += 1;
    }
    match image_index {
        0 => main_index,
        index if index <= main_index => index - 1,
        index => index,
    }
}

intercept!(_dyld_get_image_header: unsafe extern "C" fn(image_index: u32) -> *const mach_header);
unsafe extern "C" fn _dyld_get_image_header(image_index: u32) -> *const mach_header {
    // SAFETY: forwarding to the original dyld function with a remapped index.
    unsafe { _dyld_get_image_header::original()(actual_image_index(image_index)) }
}

intercept!(_dyld_get_image_vmaddr_slide: unsafe extern "C" fn(image_index: u32) -> intptr_t);
unsafe extern "C" fn _dyld_get_image_vmaddr_slide(image_index: u32) -> intptr_t {
    // SAFETY: forwarding to the original dyld function with a remapped index.
    unsafe { _dyld_get_image_vmaddr_slide::original()(actual_image_index(image_index)) }
}

intercept!(_dyld_get_image_name: unsafe extern "C" fn(image_index: u32) -> *const c_char);
unsafe extern "C" fn _dyld_get_image_name(image_index: u32) -> *const c_char {
    // SAFETY: forwarding to the original dyld function with a remapped index.
    unsafe { _dyld_get_image_name::original()(actual_image_index(image_index)) }
}
