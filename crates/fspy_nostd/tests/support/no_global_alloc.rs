#![no_std]
#![no_main]

extern crate alloc;

// Compile the actual module without needing a separate cross-built rlib.
#[path = "../../src/no_global_alloc.rs"]
mod no_global_alloc;
no_global_alloc!();

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    loop {}
}

// This is a link-only fixture, never executed. Negative cases each retain
// exactly one global allocator operation; the caller must supply a valid
// allocation for dealloc/realloc, just as for those APIs themselves.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start(pointer: *mut u8) -> ! {
    let _ = pointer;
    #[cfg(any(alloc, alloc_zeroed, dealloc, realloc))]
    let layout = core::alloc::Layout::new::<u64>();
    #[cfg(alloc)]
    core::hint::black_box(unsafe { alloc::alloc::alloc(layout) });
    #[cfg(alloc_zeroed)]
    core::hint::black_box(unsafe { alloc::alloc::alloc_zeroed(layout) });
    #[cfg(dealloc)]
    unsafe {
        alloc::alloc::dealloc(pointer, layout)
    };
    #[cfg(realloc)]
    core::hint::black_box(unsafe { alloc::alloc::realloc(pointer, layout, 16) });
    #[cfg(global_vec)]
    {
        let mut values = alloc::vec::Vec::with_capacity(core::hint::black_box(16));
        values.push(core::hint::black_box(42_usize));
        core::hint::black_box(&mut values);
        drop(values);
    }
    loop {}
}
