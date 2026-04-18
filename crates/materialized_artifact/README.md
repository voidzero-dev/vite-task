# materialized_artifact

Materialize a compile-time–embedded file to disk on demand, for APIs that
need a filesystem path (`LoadLibrary`, `LD_PRELOAD`, helper binaries) rather
than the bytes you'd get from `include_bytes!`. The on-disk filename is
content-addressed so repeated calls skip writing, multiple versions coexist,
and stale files are never mistaken for current ones. See crate-level docs
for details.
