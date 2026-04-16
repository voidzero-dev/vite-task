#[cfg(windows)]
use std::ffi::OsString;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt as _;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt as _;
#[cfg(windows)]
use std::os::windows::ffi::OsStringExt as _;
use std::{borrow::Cow, ffi::OsStr, fmt::Debug, mem::MaybeUninit};

use allocator_api2::alloc::Allocator;
#[cfg(windows)]
use bytemuck::must_cast_slice;
use bytemuck::{TransparentWrapper, TransparentWrapperAlloc};
use wincode::{
    SchemaRead, SchemaWrite,
    config::Config,
    error::{ReadResult, WriteResult},
    io::{Reader, Writer},
};

/// A platform-native string type for lossless, zero-copy IPC.
///
/// Similar to [`OsStr`], but:
/// - Can be infallibly and losslessly encoded/decoded using wincode.
///   (`SchemaWrite`/`SchemaRead` implementations for `OsStr` require it to be valid UTF-8. This does not.)
/// - Can be constructed from wide characters on Windows with zero copy.
/// - Supports zero-copy `SchemaRead`.
///
/// # Platform representation
///
/// - **Unix**: raw bytes of the `OsStr`.
/// - **Windows**: raw bytes transmuted from `&[u16]` (wide chars). See `to_os_string` for decoding.
///
/// # Limitations
///
/// **Not portable across platforms.** The binary representation is platform-specific.
/// Deserializing a `NativeStr` serialized on a different platform leads to unspecified
/// behavior (garbage data), but is not unsafe. Designed for same-platform IPC only.
#[derive(TransparentWrapper, PartialEq, Eq)]
#[repr(transparent)]
pub struct NativeStr {
    // On unix, this is the raw bytes of the OsStr.
    // On windows, this is safely transmuted from `&[u16]` in `NativeStr::from_wide`. We don't declare it as `&[u16]` to allow zero-copy read.
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

impl Debug for NativeStr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        <OsStr as Debug>::fmt(self.to_cow_os_str().as_ref(), f)
    }
}

// Manual impl: wincode derive requires Sized, but NativeStr wraps unsized [u8].
// SAFETY: Delegates to `[u8]`'s SchemaWrite impl, preserving its size/write invariants.
unsafe impl<C: Config> SchemaWrite<C> for NativeStr {
    type Src = Self;

    fn size_of(src: &Self::Src) -> WriteResult<usize> {
        <[u8] as SchemaWrite<C>>::size_of(&src.data)
    }

    fn write(writer: impl Writer, src: &Self::Src) -> WriteResult<()> {
        <[u8] as SchemaWrite<C>>::write(writer, &src.data)
    }
}

// SchemaRead for &NativeStr: zero-copy borrow from input bytes
// SAFETY: Delegates to `&[u8]`'s SchemaRead impl; dst is initialized on Ok.
unsafe impl<'de, C: Config> SchemaRead<'de, C> for &'de NativeStr {
    type Dst = &'de NativeStr;

    fn read(mut reader: impl Reader<'de>, dst: &mut MaybeUninit<Self::Dst>) -> ReadResult<()> {
        let data: &'de [u8] = <&[u8] as SchemaRead<'de, C>>::get(&mut reader)?;
        dst.write(NativeStr::wrap_ref(data));
        Ok(())
    }
}

// SAFETY: Delegates to `NativeStr`'s SchemaWrite impl, preserving its invariants.
unsafe impl<C: Config> SchemaWrite<C> for Box<NativeStr> {
    type Src = Self;

    fn size_of(src: &Self::Src) -> WriteResult<usize> {
        <NativeStr as SchemaWrite<C>>::size_of(src)
    }

    fn write(writer: impl Writer, src: &Self::Src) -> WriteResult<()> {
        <NativeStr as SchemaWrite<C>>::write(writer, src)
    }
}

// SchemaRead for Box<NativeStr>: owned decode
// SAFETY: Delegates to `&[u8]`'s SchemaRead impl; dst is initialized on Ok.
unsafe impl<'de, C: Config> SchemaRead<'de, C> for Box<NativeStr> {
    type Dst = Self;

    fn read(mut reader: impl Reader<'de>, dst: &mut MaybeUninit<Self::Dst>) -> ReadResult<()> {
        let data: &[u8] = <&[u8] as SchemaRead<'de, C>>::get(&mut reader)?;
        dst.write(NativeStr::wrap_box(data.into()));
        Ok(())
    }
}

#[cfg(unix)]
impl<'a, S: AsRef<OsStr> + ?Sized> From<&'a S> for &'a NativeStr {
    fn from(value: &'a S) -> Self {
        NativeStr::from_bytes(value.as_ref().as_bytes())
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

        let mut encoded = wincode::serialize(native_str).unwrap();

        let decoded: &NativeStr = wincode::deserialize(&encoded).unwrap();
        let decoded_wide = decoded.to_os_string().encode_wide().collect::<Vec<u16>>();
        assert_eq!(decoded_wide, wide_str);

        let encoded_len = encoded.len();
        encoded.push(0);
        encoded.copy_within(..encoded_len, 1);

        let decoded: &NativeStr = wincode::deserialize(&encoded[1..]).unwrap();
        let decoded_wide = decoded.to_os_string().encode_wide().collect::<Vec<u16>>();
        assert_eq!(decoded_wide, wide_str);
    }
}
