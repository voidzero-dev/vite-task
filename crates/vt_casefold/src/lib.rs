//! Strings that compare by a case rule while keeping their original spelling.
//!
//! [`Folded<S, R>`] wraps a string `S` and compares it by the case rule `R`.
//! `S` can be owned storage such as `Arc<OsStr>`, `OsString`, or `&str`, or an
//! unsized string such as `OsStr`: `Folded<OsStr, R>` is the borrowed form that
//! owned keys lend to maps for lookups. Equality, ordering, and hashing all use
//! the same folded bytes, so they agree across every storage type.

use std::{
    borrow::Borrow,
    cmp::Ordering,
    ffi::OsStr,
    fmt,
    hash::{Hash, Hasher},
    marker::PhantomData,
};

use ref_cast::{RefCastCustom, ref_cast_custom};

/// A rule for comparing strings.
pub trait CaseRule {
    /// Maps the encoded bytes of a string (see [`OsStr::as_encoded_bytes`]) to
    /// the bytes used for equality, ordering, and hashing.
    ///
    /// The result must depend only on `encoded`.
    fn fold(encoded: &[u8]) -> impl Iterator<Item = u8>;
}

/// Compares strings byte for byte.
#[derive(Clone, Copy, Debug)]
pub enum CaseSensitive {}

impl CaseRule for CaseSensitive {
    fn fold(encoded: &[u8]) -> impl Iterator<Item = u8> {
        encoded.iter().copied()
    }
}

/// Ignores the case of ASCII letters; all other characters must match exactly.
///
/// Letters fold to uppercase, which matches the ordering Windows uses when it
/// compares names without case.
#[derive(Clone, Copy, Debug)]
pub enum AsciiCaseInsensitive {}

impl CaseRule for AsciiCaseInsensitive {
    fn fold(encoded: &[u8]) -> impl Iterator<Item = u8> {
        // The encoding is a self-synchronizing superset of UTF-8, so ASCII
        // bytes never occur inside a multi-byte character.
        encoded.iter().map(u8::to_ascii_uppercase)
    }
}

/// Case rule for environment variable names: ASCII letters ignore case on
/// Windows.
///
/// Windows ignores the case of non-ASCII letters too, but this rule compares
/// them exactly: `état` and `ÉTAT` name the same variable on Windows but are
/// different names here.
#[cfg(windows)]
pub type EnvNameCaseRule = AsciiCaseInsensitive;

/// Case rule for environment variable names: exact on Unix.
#[cfg(not(windows))]
pub type EnvNameCaseRule = CaseSensitive;

/// An environment variable name, compared by the platform's rules.
///
/// `EnvName<OsStr>` is the borrowed form, for map lookups.
pub type EnvName<S> = Folded<S, EnvNameCaseRule>;

/// A string compared by its folded form under the case rule `R`.
///
/// Only comparisons use the folded form; the stored string keeps its original
/// spelling. `S` may be unsized. Every `Folded<S, R>` borrows as `Folded<OsStr, R>`, so
/// maps can be searched without allocating a key. `OsStr` can't serve as the
/// borrowed key, because its equality and hashing are always exact.
///
/// Inserting into a map with an existing equal key keeps the existing key's
/// spelling and replaces the value.
#[derive(Clone, Copy, RefCastCustom)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize), serde(transparent))]
#[cfg_attr(
    feature = "serde",
    expect(
        clippy::unsafe_derive_deserialize,
        reason = "the only unsafe code is the layout cast in `from_ref`, which relies on no invariant"
    )
)]
#[repr(transparent)]
pub struct Folded<S: ?Sized, R> {
    #[trivial]
    rule: PhantomData<fn() -> R>,
    inner: S,
}

impl<S, R> Folded<S, R> {
    pub const fn new(inner: S) -> Self {
        Self { rule: PhantomData, inner }
    }

    pub fn into_inner(self) -> S {
        self.inner
    }
}

impl<S: ?Sized, R> Folded<S, R> {
    /// Views a reference to a string as a reference to a `Folded` string.
    #[ref_cast_custom]
    pub const fn from_ref(inner: &S) -> &Self;

    pub const fn inner(&self) -> &S {
        &self.inner
    }
}

impl<S: AsRef<OsStr> + ?Sized, R: CaseRule> Folded<S, R> {
    fn folded(&self) -> impl Iterator<Item = u8> {
        R::fold(OsStr::as_encoded_bytes(self.inner.as_ref()))
    }

    /// Returns whether `prefix` is a prefix of this string under the case rule.
    pub fn starts_with<P: AsRef<OsStr> + ?Sized>(&self, prefix: &P) -> bool {
        let mut folded = self.folded();
        R::fold(prefix.as_ref().as_encoded_bytes()).all(|byte| folded.next() == Some(byte))
    }
}

impl<S, R> From<S> for Folded<S, R> {
    fn from(inner: S) -> Self {
        Self::new(inner)
    }
}

// Sized storage only: `Folded<OsStr, R>` already borrows as itself.
impl<S: AsRef<OsStr>, R> Borrow<Folded<OsStr, R>> for Folded<S, R> {
    fn borrow(&self) -> &Folded<OsStr, R> {
        Folded::from_ref(self.inner.as_ref())
    }
}

impl<T: ?Sized, S: AsRef<T> + ?Sized, R> AsRef<T> for Folded<S, R> {
    fn as_ref(&self) -> &T {
        self.inner.as_ref()
    }
}

impl<S1, S2, R> PartialEq<Folded<S2, R>> for Folded<S1, R>
where
    S1: AsRef<OsStr> + ?Sized,
    S2: AsRef<OsStr> + ?Sized,
    R: CaseRule,
{
    fn eq(&self, other: &Folded<S2, R>) -> bool {
        self.folded().eq(other.folded())
    }
}

impl<S: AsRef<OsStr> + ?Sized, R: CaseRule> Eq for Folded<S, R> {}

impl<S1, S2, R> PartialOrd<Folded<S2, R>> for Folded<S1, R>
where
    S1: AsRef<OsStr> + ?Sized,
    S2: AsRef<OsStr> + ?Sized,
    R: CaseRule,
{
    fn partial_cmp(&self, other: &Folded<S2, R>) -> Option<Ordering> {
        Some(self.folded().cmp(other.folded()))
    }
}

impl<S: AsRef<OsStr> + ?Sized, R: CaseRule> Ord for Folded<S, R> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.folded().cmp(other.folded())
    }
}

impl<S: AsRef<OsStr> + ?Sized, R: CaseRule> Hash for Folded<S, R> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let mut len = 0_usize;
        for byte in self.folded() {
            state.write_u8(byte);
            len += 1;
        }
        // End with the length so a string can't run into the next value when
        // several are hashed together.
        state.write_usize(len);
    }
}

impl<S: fmt::Debug + ?Sized, R> fmt::Debug for Folded<S, R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.inner, f)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        hash::BuildHasher as _,
        sync::Arc,
    };

    use rustc_hash::{FxBuildHasher, FxHashMap};

    use super::*;

    type Insensitive<S> = Folded<S, AsciiCaseInsensitive>;
    type Sensitive<S> = Folded<S, CaseSensitive>;

    fn arc(s: &str) -> Arc<OsStr> {
        Arc::from(OsStr::new(s))
    }

    #[test]
    fn ascii_rule_ignores_only_ascii_case() {
        assert_eq!(Insensitive::new("FOO"), Insensitive::new("Foo"));
        assert_eq!(Insensitive::new("FOO"), Insensitive::new("foo"));
        assert_ne!(Insensitive::new("FOO"), Insensitive::new("FOO_"));
        assert_ne!(Insensitive::new("\u{c4}"), Insensitive::new("\u{e4}"));
    }

    #[test]
    fn sensitive_rule_compares_bytes() {
        assert_eq!(Sensitive::new("FOO"), Sensitive::new("FOO"));
        assert_ne!(Sensitive::new("FOO"), Sensitive::new("Foo"));
    }

    #[test]
    fn storage_types_compare_with_each_other() {
        assert_eq!(Insensitive::new("path"), Insensitive::new(arc("PATH")));
        assert_eq!(Insensitive::new(OsStr::new("Path")), Insensitive::new(arc("PATH")));
        assert_eq!(*Insensitive::from_ref("Path"), *Insensitive::from_ref(OsStr::new("PATH")));
    }

    #[test]
    fn as_ref_forwards_to_storage() {
        let name = Insensitive::new("Path");
        assert_eq!(AsRef::<str>::as_ref(&name), "Path");
        assert_eq!(AsRef::<OsStr>::as_ref(&name), "Path");
    }

    #[test]
    fn hash_matches_borrowed_form() {
        let owned = Insensitive::new(arc("Path"));
        let borrowed = Insensitive::from_ref(OsStr::new("PATH"));
        assert_eq!(FxBuildHasher.hash_one(&owned), FxBuildHasher.hash_one(borrowed));

        let owned = Sensitive::new(arc("Path"));
        let borrowed = Sensitive::from_ref(OsStr::new("Path"));
        assert_eq!(FxBuildHasher.hash_one(&owned), FxBuildHasher.hash_one(borrowed));
    }

    #[test]
    fn map_insert_keeps_first_spelling_and_last_value() {
        let mut envs = FxHashMap::<Insensitive<Arc<OsStr>>, &str>::default();
        envs.insert(Insensitive::new(arc("Path")), "first");
        envs.insert(Insensitive::new(arc("PATH")), "last");
        assert_eq!(envs.len(), 1);

        let (name, value) = envs.get_key_value(Folded::from_ref(OsStr::new("path"))).unwrap();
        assert_eq!(AsRef::<OsStr>::as_ref(name), "Path");
        assert_eq!(*value, "last");

        envs.insert(Insensitive::new(arc("force_color")), "0");
        envs.entry(Insensitive::new(arc("FORCE_COLOR"))).or_insert("1");
        assert_eq!(envs[Folded::from_ref(OsStr::new("FORCE_COLOR"))], "0");
        assert_eq!(envs.len(), 2);
    }

    #[test]
    fn btree_map_lookup_uses_borrowed_form() {
        let envs: BTreeMap<_, _> =
            [(Insensitive::new(arc("Path")), 1), (Insensitive::new(arc("TEMP")), 2)].into();
        assert_eq!(envs.get(Folded::from_ref(OsStr::new("PATH"))), Some(&1));
        assert_eq!(envs.get(Folded::from_ref(OsStr::new("temp"))), Some(&2));
    }

    #[test]
    fn ordering_follows_rule() {
        let names = ["b", "_x", "A"];

        // Uppercase folding puts `_` after letters, as Windows does.
        let sorted: BTreeSet<_> = names.into_iter().map(Insensitive::new).collect();
        let sorted: Vec<_> = sorted.into_iter().map(Folded::into_inner).collect();
        assert_eq!(sorted, ["A", "b", "_x"]);

        let sorted: BTreeSet<_> = names.into_iter().map(Sensitive::new).collect();
        let sorted: Vec<_> = sorted.into_iter().map(Folded::into_inner).collect();
        assert_eq!(sorted, ["A", "_x", "b"]);
    }

    #[test]
    fn starts_with_follows_rule() {
        assert!(Insensitive::new("Vite_Mode").starts_with("VITE_"));
        assert!(!Sensitive::new("Vite_Mode").starts_with("VITE_"));
        assert!(Sensitive::new("VITE_MODE").starts_with("VITE_"));
        assert!(!Insensitive::new("VITE").starts_with("VITE_"));
    }

    #[test]
    fn non_unicode_bytes_compare_exactly() {
        #[cfg(unix)]
        let (upper, lower, other) = {
            use std::os::unix::ffi::OsStrExt as _;
            (
                OsStr::from_bytes(b"\xffA").to_owned(),
                OsStr::from_bytes(b"\xffa").to_owned(),
                OsStr::from_bytes(b"\xfeA").to_owned(),
            )
        };
        #[cfg(windows)]
        let (upper, lower, other) = {
            use std::{ffi::OsString, os::windows::ffi::OsStringExt as _};
            (
                OsString::from_wide(&[0xd800, u16::from(b'A')]),
                OsString::from_wide(&[0xd800, u16::from(b'a')]),
                OsString::from_wide(&[0xdc00, u16::from(b'A')]),
            )
        };
        assert_eq!(Insensitive::new(&upper), Insensitive::new(&lower));
        assert_ne!(Insensitive::new(&upper), Insensitive::new(&other));
        assert_ne!(Sensitive::new(&upper), Sensitive::new(&lower));
    }

    #[cfg(feature = "serde")]
    #[test]
    #[expect(clippy::disallowed_types, reason = "String is the simplest owned storage for tests")]
    fn serializes_as_storage() {
        let name = Insensitive::new(String::from("Path"));
        assert_eq!(serde_json::to_string(&name).unwrap(), r#""Path""#);
        assert_eq!(serde_json::to_string(Insensitive::from_ref("Path")).unwrap(), r#""Path""#);
    }

    #[cfg(feature = "serde")]
    #[test]
    #[expect(clippy::disallowed_types, reason = "String is the simplest owned storage for tests")]
    fn deserializes_with_original_spelling() {
        let name: Insensitive<String> = serde_json::from_str(r#""Path""#).unwrap();
        assert_eq!(name.inner(), "Path");
        assert_eq!(name, Insensitive::new("PATH"));
    }

    #[test]
    fn env_names_use_platform_rule() {
        assert_eq!(EnvName::new("Path") == EnvName::new("PATH"), cfg!(windows));
        assert_eq!(*EnvName::from_ref(OsStr::new("PATH")), *EnvName::from_ref(OsStr::new("PATH")));
    }
}
