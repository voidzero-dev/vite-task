# vendor_shared_memory

Vendored copy of [`shared_memory`](https://github.com/elast0ny/shared_memory-rs) v0.12.4 with a patch
that bumps `nix` from 0.23 to 0.30 so the crate compiles for `*-unknown-linux-ohos` targets.

This crate keeps the upstream package name `shared_memory` so workspace consumers (`fspy_shared`)
import it unchanged.

## Why vendor

`shared_memory 0.12.4` pins `nix = "0.23"`. `nix 0.23.x` does not gate a handful of syscalls and
constants on `target_env = "ohos"` (`aio_*`, `lio_listio`, `FDPIC_FUNCPTRS`, `UNAME26`,
`__fsword_t`, `ST_RELATIME`, etc.), so it fails to build for `aarch64-unknown-linux-ohos`.

`nix 0.30` adds the missing OHOS gates and reworks the fd-borrowing API. The patched source uses
the new API surface: `shm_open` returns `OwnedFd` (converted via `into_raw_fd`); `ftruncate`,
`fstat`, and `mmap` take `BorrowedFd`; `mmap` takes `Option<NonZeroUsize>` for size and `NonNull`
for `addr`; `munmap` takes `NonNull`.

## Patch source

The patch lives in HarmonyBrew tap at
[`Patches/shared_memory@0.12.4/0001-ohos-nix-030.patch`](https://github.com/Harmonybrew/homebrew-core/blob/main/Patches/shared_memory%400.12.4/0001-ohos-nix-030.patch).
Both this vendor crate and the brew formula consume the same patch so OHOS support stays in lockstep.

## Exit condition

Delete this crate as soon as upstream `shared_memory` ships a release with `nix` ≥ 0.30
(tracked at <https://github.com/elast0ny/shared_memory-rs>). When that happens:

1. In the workspace `Cargo.toml`, change
   `shared_memory = { path = "crates/vendor_shared_memory" }` back to
   `shared_memory = "<new-version>"`.
2. Remove this directory.
3. Drop the `Patches/shared_memory@0.12.4/` patch from the HarmonyBrew tap.

## What was omitted

The vendor crate keeps only `src/` (the library code) and the changelog. Upstream's `tests/` and
`examples/` directories — and their dev-dependencies (`raw_sync`, `clap 3`, `env_logger`) — are
left out because they're not required for `fspy_shared` to consume the library and they would pull
old transitive deps into the workspace.

## License

Upstream license: `MIT OR Apache-2.0` (see the `license` field in `Cargo.toml`).
Upstream copyright: ElasT0ny &lt;elast0ny00@gmail.com&gt;.
