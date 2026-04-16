#[expect(clippy::disallowed_types, reason = "std HashMap needed for wincode/serde compatibility")]
pub type HashMap<K, V> = std::collections::HashMap<K, V>;
