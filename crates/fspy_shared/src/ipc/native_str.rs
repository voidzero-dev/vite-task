#[cfg(windows)]
use std::ffi::OsString;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt as _;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt as _;
#[cfg(windows)]
use std::os::windows::ffi::OsStringExt as _;
use std::{borrow::Cow, ffi::OsStr, fmt::Debug};

use allocator_api2::alloc::Allocator;
use std::mem::MaybeUninit;
#[cfg(windows)]
use bytemuck::must_cast_slice;
use bytemuck::{TransparentWrapper, TransparentWrapperAlloc};

/// Similar to `OsStr`, but
/// - Can be infallibly and losslessly encoded/decoded using wincode.
///   (`Encode`/`Decoded` implementations for `OsStr` requires it to be valid UTF-8. This does not.)
/// - Can be constructed from wide characters on Windows with zero copy.
/// - Supports zero-copy `BorrowDecode`.
#[derive(TransparentWrapper, PartialEq, Eq)]
#[repr(transparent)]
pub struct NativeStr {
    // On unix, this is the raw bytes of the OsStr.
    // On windows, this is safely transmuted from `&[u16]` in `NativeStr::from_wide`. We don't declare it as `&[u16]` to allow `BorrowDecode`.
    // Transmuting back to `&[u16]` would be unsafe because of different alignments between `u8` and `u16` (See `to_os_string`).
    data: [u8],
}

impl NativeStr {
    #[cfg(unix)]
    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> &Self {
        Self::wrap_ref(bytes)
    }

    #[cfg(windows)]
    #[must_use]
    pub fn from_wide(wide: &[u16]) -> &Self {
        Self::wrap_ref(must_cast_slice(wide))
    }

    #[cfg(unix)]
    #[must_use]
    pub fn as_os_str(&self) -> &OsStr {
        OsStr::from_bytes(&self.data)
    }

    #[cfg(windows)]
    #[must_use]
    pub fn to_os_string(&self) -> OsString {
        use bytemuck::{allocation::pod_collect_to_vec, try_cast_slice};

        try_cast_slice::<u8, u16>(&self.data).map_or_else(
            |_| {
                let wide = pod_collect_to_vec::<u8, u16>(&self.data);
                OsString::from_wide(&wide)
            },
            OsString::from_wide,
        )
    }

    #[must_use]
    pub fn to_cow_os_str(&self) -> Cow<'_, OsStr> {
        #[cfg(windows)]
        return Cow::Owned(self.to_os_string());
        #[cfg(unix)]
        return Cow::Borrowed(self.as_os_str());
    }
}

impl Debug for NativeStr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        <OsStr as Debug>::fmt(self.to_cow_os_str().as_ref(), f)
    }
}

// SAFETY: NativeStr is repr(transparent) over [u8], so it serializes identically to [u8].
unsafe impl<C: wincode::config::Config> wincode::SchemaWrite<C> for NativeStr {
    type Src = NativeStr;

    fn size_of(src: &NativeStr) -> wincode::WriteResult<usize> {
        <[u8] as wincode::SchemaWrite<C>>::size_of(&src.data)
    }

    fn write(writer: impl wincode::io::Writer, src: &NativeStr) -> wincode::WriteResult<()> {
        <[u8] as wincode::SchemaWrite<C>>::write(writer, &src.data)
    }
}

// SAFETY: We borrow a &[u8] from the reader and wrap it as &NativeStr.
// dst is always initialized on success.
unsafe impl<'de, C: wincode::config::Config> wincode::SchemaRead<'de, C> for &'de NativeStr {
    type Dst = &'de NativeStr;

    fn read(
        reader: impl wincode::io::Reader<'de>,
        dst: &mut MaybeUninit<&'de NativeStr>,
    ) -> wincode::ReadResult<()> {
        let data: &'de [u8] = <&'de [u8] as wincode::SchemaRead<'de, C>>::get(reader)?;
        dst.write(NativeStr::wrap_ref(data));
        Ok(())
    }
}

#[cfg(unix)]
impl<'a, S: AsRef<OsStr> + ?Sized> From<&'a S> for &'a NativeStr {
    fn from(value: &'a S) -> Self {
        NativeStr::from_bytes(value.as_ref().as_bytes())
    }
}

// SAFETY: Box<NativeStr> serializes identically to NativeStr (which wraps [u8]).
unsafe impl<C: wincode::config::Config> wincode::SchemaWrite<C> for Box<NativeStr> {
    type Src = Box<NativeStr>;

    fn size_of(src: &Box<NativeStr>) -> wincode::WriteResult<usize> {
        <NativeStr as wincode::SchemaWrite<C>>::size_of(src)
    }

    fn write(writer: impl wincode::io::Writer, src: &Box<NativeStr>) -> wincode::WriteResult<()> {
        <NativeStr as wincode::SchemaWrite<C>>::write(writer, src)
    }
}

// SAFETY: We read a Box<[u8]> and wrap it as Box<NativeStr>.
// dst is always initialized on success.
unsafe impl<'de, C: wincode::config::Config> wincode::SchemaRead<'de, C> for Box<NativeStr> {
    type Dst = Box<NativeStr>;

    fn read(
        reader: impl wincode::io::Reader<'de>,
        dst: &mut MaybeUninit<Box<NativeStr>>,
    ) -> wincode::ReadResult<()> {
        let data: Box<[u8]> = <Box<[u8]> as wincode::SchemaRead<'de, C>>::get(reader)?;
        dst.write(NativeStr::wrap_box(data));
        Ok(())
    }
}

impl Clone for Box<NativeStr> {
    fn clone(&self) -> Self {
        NativeStr::wrap_box(self.data.into())
    }
}

impl<S: AsRef<OsStr>> From<S> for Box<NativeStr> {
    #[cfg(unix)]
    fn from(value: S) -> Self {
        NativeStr::wrap_box(value.as_ref().as_bytes().into())
    }

    #[cfg(windows)]
    fn from(value: S) -> Self {
        let wide: Vec<u16> = value.as_ref().encode_wide().collect();
        let data: &[u8] = must_cast_slice(&wide);
        NativeStr::wrap_box(data.into())
    }
}

impl NativeStr {
    pub fn clone_in<'new_alloc, A>(&self, alloc: &'new_alloc A) -> &'new_alloc Self
    where
        &'new_alloc A: Allocator,
    {
        use allocator_api2::vec::Vec;
        let mut data = Vec::<u8, _>::with_capacity_in(self.data.len(), alloc);
        data.extend_from_slice(&self.data);
        let data = data.leak::<'new_alloc>();
        Self::wrap_ref(data)
    }
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    use super::*;

    #[cfg(windows)]
    #[test]
    fn test_from_wide() {
        use std::os::windows::ffi::OsStrExt;

        let wide_str: &[u16] = &[528, 491];
        let native_str = NativeStr::from_wide(wide_str);

        let encoded = wincode::serialize(native_str).unwrap();

        let decoded: &NativeStr = wincode::deserialize(&encoded).unwrap();
        let decoded_wide = decoded.to_os_string().encode_wide().collect::<Vec<u16>>();
        assert_eq!(decoded_wide, wide_str);
    }
}
