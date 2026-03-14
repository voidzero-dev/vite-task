#[cfg(unix)]
use std::os::unix::ffi::OsStrExt as _;
use std::{
    ffi::OsStr,
    fmt::Debug,
    path::{Path, StripPrefixError},
};

use allocator_api2::alloc::Allocator;
use std::mem::MaybeUninit;
use bytemuck::TransparentWrapper;

use super::native_str::NativeStr;

/// An opaque path type used in [`super::PathAccess`].
///
/// On Windows, tracked paths are NT Object Manager paths (`\??` prefix),
/// whose raw data is not meaningful for direct consumption. The only way
/// to use the path is through [`strip_path_prefix`](NativePath::strip_path_prefix),
/// which normalizes platform differences and extracts a workspace-relative path.
#[derive(TransparentWrapper, PartialEq, Eq)]
#[repr(transparent)]
pub struct NativePath {
    inner: NativeStr,
}

impl NativePath {
    #[cfg(windows)]
    #[must_use]
    pub fn from_wide(wide: &[u16]) -> &Self {
        Self::wrap_ref(NativeStr::from_wide(wide))
    }

    pub fn clone_in<'new_alloc, A>(&self, alloc: &'new_alloc A) -> &'new_alloc Self
    where
        &'new_alloc A: Allocator,
    {
        Self::wrap_ref(self.inner.clone_in(alloc))
    }

    pub fn strip_path_prefix<P: AsRef<Path>, R, F: FnOnce(Result<&Path, StripPrefixError>) -> R>(
        &self,
        base: P,
        f: F,
    ) -> R {
        /// Strip the `\\?\`, `\\.\`, `\??\` prefix from a Windows path, if present.
        /// Does nothing on non-Windows platforms.
        ///
        /// \\?\ and \\.\ are used to enable long paths and access to device paths.
        /// \??\ is used in Nt* calls.
        /// The resulting path is not necessarily valid or points to the same location,
        /// but it's good enough for sanitizing paths in `NativePath::strip_path_prefix`.
        #[cfg_attr(
            not(windows),
            expect(
                clippy::missing_const_for_fn,
                reason = "uses non-const for loop and strip_prefix on Windows"
            )
        )]
        fn strip_windows_path_prefix(p: &OsStr) -> &OsStr {
            #[cfg(windows)]
            {
                use os_str_bytes::OsStrBytesExt as _;
                for prefix in [r"\\?\", r"\\.\", r"\??\"] {
                    if let Some(stripped) = p.strip_prefix(prefix) {
                        return stripped;
                    }
                }
                p
            }
            #[cfg(not(windows))]
            {
                p
            }
        }

        let me = self.inner.to_cow_os_str();
        let me = strip_windows_path_prefix(&me);
        let base = strip_windows_path_prefix(base.as_ref().as_os_str());
        f(Path::new(me).strip_prefix(base))
    }
}

impl Debug for NativePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        <NativeStr as Debug>::fmt(&self.inner, f)
    }
}

// SAFETY: NativePath is repr(transparent) over NativeStr, so it serializes identically.
unsafe impl<C: wincode::config::Config> wincode::SchemaWrite<C> for NativePath {
    type Src = NativePath;

    fn size_of(src: &NativePath) -> wincode::WriteResult<usize> {
        <NativeStr as wincode::SchemaWrite<C>>::size_of(&src.inner)
    }

    fn write(writer: impl wincode::io::Writer, src: &NativePath) -> wincode::WriteResult<()> {
        <NativeStr as wincode::SchemaWrite<C>>::write(writer, &src.inner)
    }
}

// SAFETY: We borrow a &NativeStr from the reader and wrap it as &NativePath.
// dst is always initialized on success.
unsafe impl<'de, C: wincode::config::Config> wincode::SchemaRead<'de, C> for &'de NativePath {
    type Dst = &'de NativePath;

    fn read(
        reader: impl wincode::io::Reader<'de>,
        dst: &mut MaybeUninit<&'de NativePath>,
    ) -> wincode::ReadResult<()> {
        let inner: &'de NativeStr =
            <&'de NativeStr as wincode::SchemaRead<'de, C>>::get(reader)?;
        dst.write(NativePath::wrap_ref(inner));
        Ok(())
    }
}

#[cfg(unix)]
impl<'a, S: AsRef<OsStr> + ?Sized> From<&'a S> for &'a NativePath {
    fn from(value: &'a S) -> Self {
        NativePath::wrap_ref(NativeStr::from_bytes(value.as_ref().as_bytes()))
    }
}
