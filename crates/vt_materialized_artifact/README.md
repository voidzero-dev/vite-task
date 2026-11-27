# vt_materialized_artifact

Materialize a compile-time–embedded file to disk on demand, for APIs that
need a filesystem path (`LoadLibrary`, `dlopen`, helper binaries) rather than
the bytes you'd get from `include_bytes!`. The on-disk filename is
content-addressed so repeated calls skip writing, multiple versions coexist,
and stale files are never mistaken for current ones. `at_user_temp_dir` falls
back from the system temp directory to the per-user cache directory when the
temp directory is unwritable or (on Unix) mounted `noexec`, keeping all
artifacts in a single usable directory. See crate-level docs for details.
