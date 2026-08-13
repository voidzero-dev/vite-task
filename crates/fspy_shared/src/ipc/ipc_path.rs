#[cfg(unix)]
use std::ffi::OsStr;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt as _;
use std::{
    fmt::Debug,
    mem::MaybeUninit,
    path::{Path, StripPrefixError},
};

use bumpalo::Bump;
use bytemuck::TransparentWrapper;
use fspy_ipc_str::IpcStr;
use wincode::{
    SchemaRead, SchemaWrite,
    config::Config,
    error::{ReadResult, WriteResult},
    io::{Reader, Writer},
};

/// An opaque path type used in [`super::PathAccess`].
///
/// On Windows, tracked paths are NT Object Manager paths (`\??` prefix),
/// whose raw data is not meaningful for direct consumption. The only way
/// to use the path is through [`strip_path_prefix`](IpcPath::strip_path_prefix),
/// which normalizes platform differences and extracts a workspace-relative path.
#[derive(TransparentWrapper, PartialEq, Eq)]
#[repr(transparent)]
pub struct IpcPath {
    inner: IpcStr,
}

// Manual impl: wincode derive requires Sized, but IpcPath wraps unsized IpcStr.
// SAFETY: Delegates to `IpcStr`'s SchemaWrite impl, preserving its invariants.
unsafe impl<C: Config> SchemaWrite<C> for IpcPath {
    type Src = Self;

    fn size_of(src: &Self::Src) -> WriteResult<usize> {
        <IpcStr as SchemaWrite<C>>::size_of(&src.inner)
    }

    fn write(writer: impl Writer, src: &Self::Src) -> WriteResult<()> {
        <IpcStr as SchemaWrite<C>>::write(writer, &src.inner)
    }
}

// SAFETY: Delegates to `&IpcStr`'s SchemaRead impl; dst is initialized on Ok.
unsafe impl<'de, C: Config> SchemaRead<'de, C> for &'de IpcPath {
    type Dst = &'de IpcPath;

    fn read(mut reader: impl Reader<'de>, dst: &mut MaybeUninit<Self::Dst>) -> ReadResult<()> {
        let inner: &'de IpcStr = <&IpcStr as SchemaRead<'de, C>>::get(&mut reader)?;
        dst.write(IpcPath::wrap_ref(inner));
        Ok(())
    }
}

impl IpcPath {
    #[cfg(windows)]
    #[must_use]
    pub fn from_wide(wide: &[u16]) -> &Self {
        Self::wrap_ref(IpcStr::from_wide(wide))
    }

    pub fn clone_in<'bump>(&self, bump: &'bump Bump) -> &'bump Self {
        Self::wrap_ref(self.inner.clone_in(bump))
    }

    pub fn strip_path_prefix<P: AsRef<Path>, R, F: FnOnce(Result<&Path, StripPrefixError>) -> R>(
        &self,
        base: P,
        f: F,
    ) -> R {
        let me = self.inner.to_cow_os_str();
        f(vt_path::strip_path_prefix(&me, base.as_ref().as_os_str()))
    }
}

impl Debug for IpcPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        <IpcStr as Debug>::fmt(&self.inner, f)
    }
}

#[cfg(unix)]
impl<'a, S: AsRef<OsStr> + ?Sized> From<&'a S> for &'a IpcPath {
    fn from(value: &'a S) -> Self {
        IpcPath::wrap_ref(IpcStr::from_bytes(value.as_ref().as_bytes()))
    }
}
