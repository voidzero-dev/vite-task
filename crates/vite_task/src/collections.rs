// std HashMap needed for bincode/serde compatibility
#[expect(clippy::disallowed_types)]
pub type HashMap<K, V> = std::collections::HashMap<K, V>;
