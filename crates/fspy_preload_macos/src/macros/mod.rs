mod macos;

#[expect(
    clippy::redundant_pub_crate,
    reason = "macro_rules! macros cannot be `pub`, only `pub(crate)` at most"
)]
pub(crate) use macos::*;
