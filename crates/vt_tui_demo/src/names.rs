//! Names for runs, which `vp run --last-details` takes to show one again.

use vt_str::Str;

const ADJECTIVES: [&str; 16] = [
    "brave", "calm", "eager", "gentle", "jolly", "lucky", "quiet", "swift", "tidy", "witty",
    "bold", "bright", "mellow", "nimble", "sunny", "vivid",
];
const NOUNS: [&str; 16] = [
    "otter", "falcon", "maple", "comet", "harbor", "lynx", "pebble", "river", "sparrow", "willow",
    "ember", "tundra", "meadow", "badger", "cedar", "lagoon",
];

/// Picks names like `brave-otter` from a seed.
#[derive(Debug, Clone)]
pub struct RunNames {
    seed: u32,
}

impl RunNames {
    #[must_use]
    pub const fn new(seed: u32) -> Self {
        Self { seed: seed | 1 }
    }

    const fn next_random(&mut self) -> u32 {
        // xorshift32
        let mut x = self.seed;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.seed = x;
        x
    }

    pub fn next_name(&mut self) -> Str {
        let adjective = ADJECTIVES[self.next_random() as usize % ADJECTIVES.len()];
        let noun = NOUNS[self.next_random() as usize % NOUNS.len()];
        vt_str::format!("{adjective}-{noun}")
    }
}
