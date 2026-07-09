macro_rules! intercept {
    ($name: ident (64): $fn_sig: ty) => {
        $crate::macros::intercept_inner! {
            $name: $fn_sig;

            #[cfg(test)]
            #[test]
            fn symbol_64_exists() {
                ::core::assert!($crate::macros::symbol_exists(::core::stringify!($name)));
            }
        }
        #[cfg(not(test))] // Don't interpose on the test binary
        const _: () = {
            #[unsafe(naked)]
            #[unsafe(export_name = ::core::concat!(::core::stringify!($name), 64))]
            pub unsafe extern "C" fn interpose_fn() {
                #[cfg(target_arch = "aarch64")]
        ::core::arch::naked_asm!("b {}", sym $name);
                #[cfg(target_arch = "x86_64")]
        ::core::arch::naked_asm!("jmp {}", sym $name);
            }
        };
    };
    ($name: ident: $fn_sig: ty) => {
        $crate::macros::intercept_inner! {
            $name: $fn_sig;

            #[cfg(test)]
            #[test]
            fn symbol_64_does_not_exist() {
                ::core::assert_eq!(
                    $crate::macros::symbol_exists(::core::concat!(::core::stringify!($name), 64)),
                    false,
                );
            }
        }
    };
}

pub(crate) use intercept;

#[cfg(test)]
#[doc(hidden)]
pub fn symbol_exists(name: &str) -> bool {
    use std::ffi::CString;

    let name = CString::new(name).unwrap();
    // SAFETY: dlsym with RTLD_DEFAULT searches for the symbol in the default shared object search order
    !unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr().cast()) }.is_null()
}

macro_rules! intercept_inner {
    ($name: ident: $fn_sig: ty; $test_fn: item) => {
        const _: $fn_sig = $name;
        const _: $fn_sig = $crate::libc::$name;

        #[cfg(not(test))] // Don't interpose on the test binary
        const _: () = {
            #[unsafe(naked)]
            #[unsafe(export_name = ::core::stringify!($name))]
            pub unsafe extern "C" fn interpose_fn() {
                #[cfg(target_arch = "aarch64")]
                ::core::arch::naked_asm!("b {}", sym $name);
                #[cfg(target_arch = "x86_64")]
                ::core::arch::naked_asm!("jmp {}", sym $name);
            }
        };
        mod $name {
            #[expect(clippy::allow_attributes, reason = "using allow because unused_imports may or may not fire depending on macro expansion")]
            #[allow(unused_imports, reason = "glob import brings types into scope for macro-generated code")]
            use super::*;
            #[expect(
                clippy::allow_attributes,
                reason = "using allow because dead_code only fires for optional original symbols"
            )]
            #[allow(
                dead_code,
                reason = "not every interposer forwards to its generated original function"
            )]
            pub unsafe fn original() -> $fn_sig {
                try_original().unwrap_or_else(|| {
                    panic!(::core::concat!(
                        "original symbol not found: ",
                        ::core::stringify!($name)
                    ))
                })
            }
            pub fn try_original() -> ::core::option::Option<$fn_sig> {
                static LAZY: std::sync::LazyLock<::core::option::Option<$fn_sig>> =
                    std::sync::LazyLock::new(|| {
                        // SAFETY: dlsym with RTLD_NEXT returns the next symbol in the dynamic
                        // linking order. A non-null pointer has the signature checked by the
                        // macro invocation.
                        let symbol = unsafe {
                            ::libc::dlsym(
                                ::libc::RTLD_NEXT,
                                ::core::concat!(::core::stringify!($name), "\0").as_ptr().cast(),
                            )
                        };
                        if symbol.is_null() {
                            ::core::option::Option::None
                        } else {
                            // SAFETY: the symbol name and function signature are paired by the
                            // macro invocation, and null was checked above.
                            ::core::option::Option::Some(unsafe {
                                ::core::mem::transmute::<*mut ::libc::c_void, $fn_sig>(symbol)
                            })
                        }
                    });
                *LAZY
            }
            $test_fn
        }
    };
}

pub(crate) use intercept_inner;
