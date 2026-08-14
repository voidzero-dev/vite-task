# vt_fs_fingerprint

The filesystem story of one task run, extracted from the execution engine so
it can be reasoned about and tested on its own: what a run read, what it
produced, and whether a previous run's cached result still matches the
filesystem. Everything else about caching — storage, archiving, env tracking,
process spawning — stays with the engine; this crate is the part that decides
what filesystem facts _mean_.

The API is one typestate with a call per stage:

- **`TaskFs::pre_run`** — before the task executes: validate its io
  configuration, capture the state of its listed inputs, and (when a previous
  run's fingerprints were fetched) report the first input that changed since
  that run.
- **`TaskFs::post_run`** — after it finished: judge the traced file accesses
  and either declare caching unsound (`Conclusion::InputModified` — the task
  wrote a path it also read) or return everything the cache should remember
  (`Conclusion::Cacheable`: the run's `InputFingerprints` and its outputs).

`InputFingerprints` is the opaque record linking runs together: `post_run`
produces it, the caller stores it in the cache entry, and a later run's
`pre_run` checks the filesystem against it.

Why a separate crate: the policy for a path that a task both reads and writes
is unsettled and expected to be rewritten several times. Here it can be driven
directly with synthetic traces and temp directories, instead of only being
observable by running a whole task through the engine.

Design notes worth knowing:

- The pre-run snapshot is taken before the task runs as a **soundness**
  requirement, not a convenience: a task that modifies one of its own listed
  inputs without tracing to catch it must perpetually miss. A snapshot taken
  after the run would capture the post-run state and turn that safe perpetual
  miss into a false cache hit.
- `pre_run` is handed the previous fingerprints (fetch-first) rather than
  exposing a separate comparison method, so comparing after the run — or
  against the wrong run's record — is unrepresentable.
- "No filesystem change" from `pre_run` is not yet a cache hit: the engine
  still validates tracked environment state, which lives outside this crate.
