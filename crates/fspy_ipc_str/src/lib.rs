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
use bumpalo::Bump;
#[cfg(windows)]
use bytemuck::must_cast_slice;
use bytemuck::{TransparentWrapper, TransparentWrapperAlloc};
use fspy_nostd::{Fat, OsCStr};
use fspy_nostd_alloc::OsCString;
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
/// Deserializing an `IpcStr` serialized on a different platform leads to unspecified
/// behavior (garbage data), but is not unsafe. Designed for same-platform IPC only.
#[derive(TransparentWrapper, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct IpcStr {
    // On unix, this is the raw bytes of the OsStr.
    // On windows, this is safely transmuted from `&[u16]` in `IpcStr::from_wide`. We don't declare it as `&[u16]` to allow zero-copy read.
    // Transmuting back to `&[u16]` would be unsafe because of different alignments between `u8` and `u16` (See `to_os_string`).
    data: [u8],
}

impl IpcStr {
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

    pub fn clone_in<'bump>(&self, bump: &'bump Bump) -> &'bump Self {
        Self::wrap_ref(bump.alloc_slice_copy(&self.data))
    }

    /// Creates an IPC string that borrows the code units of `path`, without
    /// its NUL terminator.
    ///
    /// This is the inverse of [`to_os_c_string_in`](Self::to_os_c_string_in);
    /// neither direction goes through [`OsStr`], so both work without std.
    #[must_use]
    pub fn from_os_c_str(path: OsCStr<'_, Fat>) -> &Self {
        #[cfg(unix)]
        return Self::wrap_ref(path.as_units());
        #[cfg(windows)]
        return Self::wrap_ref(must_cast_slice(path.as_units()));
    }

    /// Decodes this IPC string into an owned NUL-terminated platform C
    /// string allocated in `allocator`.
    ///
    /// Returns [`None`] when the contents cannot name a path: an odd byte
    /// length on Windows, or an interior NUL code unit.
    #[must_use]
    pub fn to_os_c_string_in<A: Allocator>(&self, allocator: A) -> Option<OsCString<Fat, A>> {
        #[cfg(unix)]
        {
            let mut units =
                allocator_api2::vec::Vec::with_capacity_in(self.data.len() + 1, allocator);
            units.extend_from_slice(&self.data);
            units.push(0);
            OsCString::from_vec_with_nul(units)
        }
        #[cfg(windows)]
        {
            if !self.data.len().is_multiple_of(2) {
                return None;
            }
            let len = self.data.len() / 2;
            let mut units = allocator_api2::vec::Vec::with_capacity_in(len + 1, allocator);
            units.resize(len, 0);
            // The destination is aligned `u16` storage; viewing it as bytes
            // sidesteps the source's unspecified alignment (see the field
            // docs).
            bytemuck::must_cast_slice_mut::<u16, u8>(&mut units).copy_from_slice(&self.data);
            units.push(0);
            OsCString::from_vec_with_nul(units)
        }
    }
}

impl Debug for IpcStr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        <OsStr as Debug>::fmt(self.to_cow_os_str().as_ref(), f)
    }
}

// Manual impl: wincode derive requires Sized, but IpcStr wraps unsized [u8].
// SAFETY: Delegates to `[u8]`'s SchemaWrite impl, preserving its size/write invariants.
unsafe impl<C: Config> SchemaWrite<C> for IpcStr {
    type Src = Self;

    fn size_of(src: &Self::Src) -> WriteResult<usize> {
        <[u8] as SchemaWrite<C>>::size_of(&src.data)
    }

    fn write(writer: impl Writer, src: &Self::Src) -> WriteResult<()> {
        <[u8] as SchemaWrite<C>>::write(writer, &src.data)
    }
}

// SchemaRead for &IpcStr: zero-copy borrow from input bytes
// SAFETY: Delegates to `&[u8]`'s SchemaRead impl; dst is initialized on Ok.
unsafe impl<'de, C: Config> SchemaRead<'de, C> for &'de IpcStr {
    type Dst = &'de IpcStr;

    fn read(mut reader: impl Reader<'de>, dst: &mut MaybeUninit<Self::Dst>) -> ReadResult<()> {
        let data: &'de [u8] = <&[u8] as SchemaRead<'de, C>>::get(&mut reader)?;
        dst.write(IpcStr::wrap_ref(data));
        Ok(())
    }
}

// SAFETY: Delegates to `IpcStr`'s SchemaWrite impl, preserving its invariants.
unsafe impl<C: Config> SchemaWrite<C> for Box<IpcStr> {
    type Src = Self;

    fn size_of(src: &Self::Src) -> WriteResult<usize> {
        <IpcStr as SchemaWrite<C>>::size_of(src)
    }

    fn write(writer: impl Writer, src: &Self::Src) -> WriteResult<()> {
        <IpcStr as SchemaWrite<C>>::write(writer, src)
    }
}

// SchemaRead for Box<IpcStr>: owned decode
// SAFETY: Delegates to `&[u8]`'s SchemaRead impl; dst is initialized on Ok.
unsafe impl<'de, C: Config> SchemaRead<'de, C> for Box<IpcStr> {
    type Dst = Self;

    fn read(mut reader: impl Reader<'de>, dst: &mut MaybeUninit<Self::Dst>) -> ReadResult<()> {
        let data: &[u8] = <&[u8] as SchemaRead<'de, C>>::get(&mut reader)?;
        dst.write(IpcStr::wrap_box(data.into()));
        Ok(())
    }
}

#[cfg(unix)]
impl<'a, S: AsRef<OsStr> + ?Sized> From<&'a S> for &'a IpcStr {
    fn from(value: &'a S) -> Self {
        IpcStr::from_bytes(value.as_ref().as_bytes())
    }
}

impl<S: AsRef<OsStr>> From<S> for Box<IpcStr> {
    #[cfg(unix)]
    fn from(value: S) -> Self {
        IpcStr::wrap_box(value.as_ref().as_bytes().into())
    }

    #[cfg(windows)]
    fn from(value: S) -> Self {
        let wide: Vec<u16> = value.as_ref().encode_wide().collect();
        let data: &[u8] = must_cast_slice(&wide);
        IpcStr::wrap_box(data.into())
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
        let ipc_str = IpcStr::from_wide(wide_str);

        let mut encoded = wincode::serialize(ipc_str).unwrap();

        let decoded: &IpcStr = wincode::deserialize(&encoded).unwrap();
        let decoded_wide = decoded.to_os_string().encode_wide().collect::<Vec<u16>>();
        assert_eq!(decoded_wide, wide_str);

        let encoded_len = encoded.len();
        encoded.push(0);
        encoded.copy_within(..encoded_len, 1);

        let decoded: &IpcStr = wincode::deserialize(&encoded[1..]).unwrap();
        let decoded_wide = decoded.to_os_string().encode_wide().collect::<Vec<u16>>();
        assert_eq!(decoded_wide, wide_str);
    }
}
