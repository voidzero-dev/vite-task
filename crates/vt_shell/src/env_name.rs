use std::{ffi::OsStr, fmt, mem::MaybeUninit};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
#[cfg(windows)]
use unicase::UniCase;
use vt_str::Str;
use wincode::{
    SchemaRead, SchemaWrite,
    config::Config,
    error::{ReadResult, WriteResult},
    io::{Reader, Writer},
};

/// An environment name, compared without ASCII case on Windows.
///
/// The original spelling is preserved for display and serialization.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct EnvName(#[cfg(windows)] UniCase<Str>, #[cfg(not(windows))] Str);

impl EnvName {
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_ref()
    }
}

impl From<Str> for EnvName {
    fn from(name: Str) -> Self {
        #[cfg(windows)]
        let name = UniCase::ascii(name);
        Self(name)
    }
}

impl From<&str> for EnvName {
    fn from(name: &str) -> Self {
        Self::from(Str::from(name))
    }
}

impl AsRef<OsStr> for EnvName {
    fn as_ref(&self) -> &OsStr {
        OsStr::new(self.as_str())
    }
}

impl fmt::Display for EnvName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.as_str(), f)
    }
}

impl fmt::Debug for EnvName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl Serialize for EnvName {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.as_str().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for EnvName {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Str::deserialize(deserializer).map(Self::from)
    }
}

// SAFETY: Delegates to str's schema, preserving its size and write invariants.
unsafe impl<C: Config> SchemaWrite<C> for EnvName {
    type Src = Self;

    fn size_of(src: &Self) -> WriteResult<usize> {
        <str as SchemaWrite<C>>::size_of(src.as_str())
    }

    fn write(writer: impl Writer, src: &Self) -> WriteResult<()> {
        <str as SchemaWrite<C>>::write(writer, src.as_str())
    }
}

// SAFETY: Delegates to Str's schema and initializes dst with the wrapped result on success.
unsafe impl<'de, C: Config> SchemaRead<'de, C> for EnvName {
    type Dst = Self;

    fn read(reader: impl Reader<'de>, dst: &mut MaybeUninit<Self>) -> ReadResult<()> {
        dst.write(Self::from(<Str as SchemaRead<'de, C>>::get(reader)?));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::EnvName;

    #[test]
    fn map_uses_platform_case_rules() {
        let mut envs = BTreeMap::new();
        envs.insert(EnvName::from("Foo"), "first");
        envs.insert(EnvName::from("FOO"), "last");
        assert_eq!(envs.len(), if cfg!(windows) { 1 } else { 2 });
        assert_eq!(envs.get(&EnvName::from("FOO")), Some(&"last"));
        assert_eq!(
            envs.get_key_value(&EnvName::from("Foo")).map(|(key, value)| (key.as_str(), *value)),
            Some(("Foo", if cfg!(windows) { "last" } else { "first" })),
        );
        assert_eq!(envs.remove(&EnvName::from("foo")), cfg!(windows).then_some("last"));

        // Non-ASCII names remain distinct, even on Windows.
        assert_ne!(EnvName::from("Ä"), EnvName::from("ä"));
        assert_ne!(EnvName::from("Straße"), EnvName::from("STRASSE"));
    }
}
