#[expect(clippy::disallowed_types, reason = "vite_str defines Str using std types internally")]
use std::{
    borrow::Borrow,
    ffi::OsStr,
    fmt::{Debug, Display},
    mem::MaybeUninit,
    ops::Deref,
    path::Path,
    sync::Arc,
};

use compact_str::CompactString;
#[doc(hidden)] // for `format` macro only
pub use compact_str::format_compact;
use diff::Diff;
use serde::{Deserialize, Serialize};
use wincode::{
    SchemaRead, SchemaWrite,
    config::Config,
    error::{ReadResult, WriteResult},
    io::{Reader, Writer},
};

#[macro_export]
macro_rules! format {
    ($($arg:tt)*) => {
        $crate::Str::from($crate::format_compact!($($arg)*))
    };
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Default, Hash, PartialOrd, Ord)]
#[serde(transparent)]
pub struct Str(CompactString);

impl Diff for Str {
    type Repr = Option<Self>;

    fn diff(&self, other: &Self) -> Self::Repr {
        if self == other { None } else { Some(other.clone()) }
    }

    fn apply(&mut self, diff: &Self::Repr) {
        if let Some(diff) = diff {
            *self = diff.clone();
        }
    }

    fn identity() -> Self {
        Self::default()
    }
}

impl Str {
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self(CompactString::with_capacity(capacity))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    pub fn push(&mut self, ch: char) {
        self.0.push(ch);
    }

    pub fn pop(&mut self) -> Option<char> {
        self.0.pop()
    }

    pub fn push_str(&mut self, s: &str) {
        self.0.push_str(s);
    }
}

impl AsRef<str> for Str {
    fn as_ref(&self) -> &str {
        self.0.as_ref()
    }
}
#[expect(clippy::disallowed_types, reason = "vite_str provides Path interop via AsRef")]
impl AsRef<Path> for Str {
    #[expect(clippy::disallowed_types, reason = "fn signature uses std Path")]
    fn as_ref(&self) -> &Path {
        self.0.as_ref()
    }
}
impl AsRef<OsStr> for Str {
    fn as_ref(&self) -> &OsStr {
        self.0.as_ref()
    }
}
impl Borrow<str> for Str {
    fn borrow(&self) -> &str {
        self.0.borrow()
    }
}
impl Deref for Str {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Display for Str {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.0, f)
    }
}
impl Debug for Str {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Debug::fmt(&self.0, f)
    }
}

// SAFETY: Delegates to `str`'s SchemaWrite impl, preserving its size/write invariants.
unsafe impl<C: Config> SchemaWrite<C> for Str {
    type Src = Self;

    fn size_of(src: &Self::Src) -> WriteResult<usize> {
        <str as SchemaWrite<C>>::size_of(src.as_str())
    }

    fn write(writer: impl Writer, src: &Self::Src) -> WriteResult<()> {
        <str as SchemaWrite<C>>::write(writer, src.as_str())
    }
}

// SAFETY: Delegates to `&str`'s SchemaRead impl and wraps the result; dst is initialized on Ok.
unsafe impl<'de, C: Config> SchemaRead<'de, C> for Str {
    type Dst = Self;

    fn read(mut reader: impl Reader<'de>, dst: &mut MaybeUninit<Self::Dst>) -> ReadResult<()> {
        let s: &str = <&str as SchemaRead<'de, C>>::get(&mut reader)?;
        dst.write(Self(CompactString::from(s)));
        Ok(())
    }
}

impl From<&str> for Str {
    fn from(value: &str) -> Self {
        Self(value.into())
    }
}

#[expect(clippy::disallowed_types, reason = "vite_str provides String conversion via From")]
impl From<String> for Str {
    #[expect(clippy::disallowed_types, reason = "fn signature uses std String")]
    fn from(value: String) -> Self {
        Self(value.into())
    }
}

impl From<CompactString> for Str {
    fn from(value: CompactString) -> Self {
        Self(value)
    }
}

impl From<Str> for Arc<str> {
    fn from(value: Str) -> Self {
        Self::from(value.as_str())
    }
}

impl PartialEq<&str> for Str {
    fn eq(&self, other: &&str) -> bool {
        self.0 == other
    }
}
impl PartialEq<str> for Str {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

#[cfg(feature = "ts-rs")]
mod ts_impl {
    use ts_rs::TS;

    use super::Str;

    #[expect(clippy::disallowed_types, reason = "ts_rs::TS trait requires returning String")]
    impl TS for Str {
        type OptionInnerType = Self;
        type WithoutGenerics = Self;

        fn name() -> String {
            "string".to_owned()
        }

        fn inline() -> String {
            "string".to_owned()
        }

        fn inline_flattened() -> String {
            panic!("Str cannot be flattened")
        }

        fn decl() -> String {
            panic!("Str is a primitive type")
        }

        fn decl_concrete() -> String {
            panic!("Str is a primitive type")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_str_encode_decode() {
        let original = Str::from("Hello, World!");
        let encoded = wincode::serialize(&original).unwrap();
        let decoded: Str = wincode::deserialize(&encoded).unwrap();
        assert_eq!(original, decoded);
    }
}
