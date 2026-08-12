use std::{cell::UnsafeCell, ffi::CStr, mem::transmute_copy, os::raw::c_void, ptr::null_mut};

use fspy_detours_sys::{DetourAttach, DetourDetach};
use fspy_nostd::{ModuleHandle, get_module_name, wide_cstr};
use winapi::um::libloaderapi::GetProcAddress;
use winsafe::SysResult;

use crate::windows::winapi_utils::ck_long;

// SAFETY: Detour<T> is only mutated during DLL attach/detach (single-threaded DLL_PROCESS_ATTACH)
unsafe impl<T: Sync> Sync for Detour<T> {}
pub struct Detour<T> {
    symbol_name: &'static CStr,
    target: UnsafeCell<*mut c_void>,
    new: T,
}

impl<T: Copy> Detour<T> {
    pub const unsafe fn new(symbol_name: &'static CStr, target: T, new: T) -> Self {
        // SAFETY: transmute_copy reinterprets the function pointer as *mut c_void for Detours API
        Self { symbol_name, target: UnsafeCell::new(unsafe { transmute_copy(&target) }), new }
    }

    pub const unsafe fn dynamic(symbol_name: &'static CStr, new: T) -> Self {
        Self { symbol_name, target: UnsafeCell::new(null_mut()), new }
    }

    #[must_use]
    pub fn real(&self) -> &T {
        // SAFETY: target is initialized during Detour construction or attach; read-only after attach
        unsafe { &(*self.target.get().cast::<T>()) }
    }

    pub const fn as_any(&'static self) -> DetourAny
    where
        T: Copy,
    {
        DetourAny {
            symbol_name: std::ptr::addr_of!(self.symbol_name),
            target: self.target.get(),
            new: (&raw const self.new).cast(),
        }
    }
}

#[derive(Clone, Copy)]
pub struct DetourAny {
    symbol_name: *const &'static CStr,
    target: *mut *mut c_void,
    new: *const *mut c_void,
}

pub struct AttachContext {
    kernelbase: Option<ModuleHandle>,
    kernel32: ModuleHandle,
    ntdll: ModuleHandle,
}

impl AttachContext {
    pub fn new() -> fspy_nostd::Result<Self> {
        let kernelbase = get_module_name(wide_cstr!("kernelbase.dll")).ok();
        let kernel32 = get_module_name(wide_cstr!("kernel32.dll"))?;
        let ntdll = get_module_name(wide_cstr!("ntdll.dll"))?;
        Ok(Self { kernelbase, kernel32, ntdll })
    }
}

// SAFETY: DetourAny is only used during DLL attach/detach (single-threaded DLL_PROCESS_ATTACH)
unsafe impl Sync for DetourAny {}
impl DetourAny {
    pub unsafe fn attach(&self, ctx: &AttachContext) -> SysResult<()> {
        // SAFETY: dereferencing pointer to static CStr symbol name
        let symbol_name = unsafe { *self.symbol_name }.as_ptr();
        if let Some(kernelbase) = ctx.kernelbase {
            // SAFETY: GetProcAddress FFI call with valid module handle and symbol name
            let symbol_in_kernelbase =
                unsafe { GetProcAddress(kernelbase.as_ptr().cast(), symbol_name) };
            if !symbol_in_kernelbase.is_null() {
                // Stub symbols in kernel32 and other DLLs forward here. Hooking
                // the shared implementation covers every stub.
                // https://github.com/microsoft/Detours/issues/328#issuecomment-2494147615
                // SAFETY: writing resolved symbol address to target pointer for Detours API
                unsafe { *self.target = symbol_in_kernelbase.cast() };
            }
        }
        // SAFETY: reading target pointer to check if the statically imported
        // target was resolved or KernelBase supplied an implementation.
        if unsafe { *self.target }.is_null() {
            // Dynamic symbols may come from kernel32 or ntdll.
            // SAFETY: GetProcAddress FFI call with valid module handle and symbol name
            let symbol_in_kernel32 =
                unsafe { GetProcAddress(ctx.kernel32.as_ptr().cast(), symbol_name) };
            if symbol_in_kernel32.is_null() {
                // SAFETY: GetProcAddress FFI call with valid module handle and symbol name
                let symbol_in_ntdll =
                    unsafe { GetProcAddress(ctx.ntdll.as_ptr().cast(), symbol_name) };
                // SAFETY: writing resolved symbol address to target pointer
                unsafe { *self.target = symbol_in_ntdll.cast() };
            } else {
                // SAFETY: writing resolved symbol address to target pointer
                unsafe { *self.target = symbol_in_kernel32.cast() };
            }
        }
        // SAFETY: reading target pointer to check if symbol was resolved
        if unsafe { *self.target }.is_null() {
            // dynamic symbol not found, skip attaching
            return Ok(());
        }
        // SAFETY: DetourAttach FFI call with valid target and detour function pointers
        ck_long(unsafe { DetourAttach(self.target, *self.new) })?;
        Ok(())
    }

    pub unsafe fn detach(&self) -> SysResult<()> {
        // SAFETY: reading target pointer to check if symbol was resolved
        if unsafe { *self.target }.is_null() {
            // dynamic symbol not found, skip detaching
            return Ok(());
        }
        // SAFETY: DetourDetach FFI call with valid target and detour function pointers
        ck_long(unsafe { DetourDetach(self.target, *self.new) })
    }
}
