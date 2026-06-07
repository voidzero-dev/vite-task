# Changelog

# 0.12.4
- Allow Windows users to open shared memory that isnt managed by this crate
- Added tests

# 0.12.2
- Default feature behavior is to disable logging on release builds
- Reverted edition bump back to 2018
- Updated to use Microsoft's `windows-rs` crate

# ~~0.12.1 (yanked)~~
- ~~Updated to latest edition (2021)~~

# 0.12.0
- Windows implementation now follows POSIX behavior in regards to ownership and deletion, see [#59](https://github.com/elast0ny/shared_memory-rs/pull/59) for more details
# __0.11.X__
This release breaks backwards compatibility and removes a bunch of previous features which hid many unsafe behaviors (automatically casting shared memory to Rust types).

The release also marks the split between `shared_memory` and its synchronization primitives into a seperate crate `raw_sync`.