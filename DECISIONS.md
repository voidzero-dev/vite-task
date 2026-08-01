# Decision log

Decisions taken while implementing the two auto-tracking rule sets
(`target/fspy-overlap-experiments/read-write-overlaps.md` and
`output-cleanup.md`) and wiring them through vite-plus into emdash.

Newest entries at the bottom. Each entry records the drift, the choice, and why.

## D1 — Tracer scope: all signals, all three platforms

Confirmed with the requester before starting. The full rule set needs fspy to
gain open success/errno, `O_CREAT`/`O_TRUNC`/`O_EXCL`, confirmed writes, rename
source and destination, unlink/rmdir, and mkdir with errno — on the Unix
preload, the Linux seccomp fallback, and the Windows detours. No platform is
stubbed. `AGENTS.md` forbids skipping tests on either platform.

## D2 — Rules implemented in place, no crate extraction

Confirmed before starting. The `vite_task_fs_cache` extraction stays a separate
follow-up so it keeps its no-behavior-change property. This PR changes behavior
and would obscure that proof.

## D3 — Gitignore via the `ignore` crate

Confirmed before starting. Hand-rolling gitignore precedence, negation and
nesting is easy to get subtly wrong, and shelling out to `git check-ignore` puts
a process spawn on a hot path and fails when git is absent.

## D4 — vite-plus preview PR lives on the upstream repo

Confirmed before starting. `publish-preview.yml` gates on
`github.repository == 'voidzero-dev/vite-plus'` plus a `preview-build` label, so
a fork cannot produce a pkg.pr.new build. Branch is pushed to the upstream repo;
the PR stays a draft and is not merged.

## D5 — Approximate "confirmed write" from open flags, do not intercept `write`

**Drift.** `read-write-overlaps.md` defines a write as a confirmed
`write`/`pwrite`/`writev`/`ftruncate` on a write-opened descriptor. Implementing
that needs a per-process fd table plus interception of `close`, `dup`, `dup2` and
`dup3`, because a descriptor number can otherwise be reacquired by an owner the
tracer never saw. I hit exactly that bug in the experiment probe, where
`vite.config.mjs` looked written because fd 18 was reused.

**Decision.** Record open success and the `O_CREAT`, `O_TRUNC`, `O_EXCL` flags
instead, and treat a mutation as `O_TRUNC`, or `O_CREAT | O_EXCL`, or a rename
destination. A bare write-access open is recorded as capability only and is not
a mutation.

**Why this is sufficient.** Checked against every case in the experiment matrix:

- Prettier, ESLint, Stylelint, Vite, Astro all rewrite through
  `O_CREAT | O_TRUNC`, so they are still detected.
- Atomic writers create temporaries with `O_CREAT | O_EXCL` and publish by
  rename, so both halves are still detected.
- Biome's warm run opens a clean source `O_RDWR` with no truncation and does not
  write. It is now correctly _not_ a mutation, which is the false positive the
  rule exists to remove.
- Lock files across cargo, rustc and Parcel are opened `O_RDWR | O_CREAT`
  without truncation and only flocked, so they stop being collected.

**What it gives up.** Mutation through a writable mmap is invisible. The only
instance in the matrix is Parcel's `lock.mdb`, a lock file that should not be
archived anyway. Recorded as a known gap rather than fixed.

## D6 — Skip `getdents` success; use mkdir instead

**Drift.** `output-cleanup.md` lists "errno on getdents or scandir" as the way to
tell that a directory listing failed, which matters for the
`unconditional-clean` case where a tool lists a directory that does not exist.

**Decision.** Do not add it. Fix 3, "a directory the task created is not an
input", covers the same case using successful `mkdir`, which is far cheaper to
intercept and which the experiment already showed absorbs 264 of 307 derived
directory listings including all 256 of Go's cache shards.

## D7 — The Linux seccomp fallback is a reduced-fidelity path

**Drift.** D1 committed to all signals on all three backends. The seccomp
supervisor responds with `SECCOMP_USER_NOTIF_FLAG_CONTINUE`
(`fspy_seccomp_unotify/src/supervisor/listener.rs:50`), so it is notified
_before_ the syscall runs and never learns the result. Success-dependent signals
are therefore not obtainable there without emulating each syscall in the
supervisor, which would mean reproducing the target's cwd, dirfd and credentials.

**Decision.** On the seccomp path emit only what is knowable before the call:
read/write intent plus `CREATE`, `TRUNCATE` and `EXCLUSIVE` from the open flags.
Do **not** emit `FAILED`, `DELETED`, `CREATED_DIR` or the rename pair, because
emitting them unconditionally would be actively wrong — a failed `mkdir` would
claim a pre-existing directory, and a failed `unlink` would retire a path that is
still there.

This path is the fallback for statically linked binaries where preload injection
does not apply; the primary Linux path is the preload library, which has the full
set.

**Why the missing signals are safe.** Every one degrades toward caching less:
absent `DELETED` leaves a listing as an input, absent `CREATED_DIR` leaves a
directory as an input. Both cost cache hits, not correctness.

## D8 — Guard: an unexplained missing mutation blocks caching

**Drift.** One missing signal is _not_ safe on its own. If a rename goes
unobserved, the write lands on a staging path that no longer exists at archive
time, rule 6 drops it, and the archive comes out empty — which restores nothing
on a cache hit and leaves a wrong tree. That is exactly the `atomic-dist-swap`
failure, and it would resurface on the seccomp path.

**Decision.** Add a platform-agnostic guard in `vite_task`: if a recorded
mutation targets a path that is absent at archive time and no observed rename
explains where it went, the output set is incomplete, so refuse to cache the run.

This protects correctness beyond the seccomp case. It also covers mutations
through `clonefile`, `copyfile` and `FICLONE`, none of which is intercepted, and
any future publish mechanism the tracer has not learned about yet. The cost is a
missed cache entry, never a wrong tree.

## D9 — Never rely on syscall outcomes; supersedes D5, D7 and D8

**Instruction.** Do not build rules on whether a call succeeded.

**What this invalidates.** D1 through D8 assumed the tracer could report
outcomes, which drove reporting _after_ the real call so `errno` was known. That
is now reverted:

- `AccessMode::FAILED` is removed, along with the after-the-call reporting in
  `open.rs`, `stat.rs`, `access.rs` and `dirent.rs`. They report before the call
  again, as they did originally.
- `CREATED_DIR` is removed. Recording it pre-call would claim every directory a
  caller merely ensured exists, since `mkdir` returning `EEXIST` is the normal
  case. Output-cleanup fix 3 therefore cannot be implemented.
- Output-cleanup fix 2, dropping a listing whose entries were then deleted, also
  cannot be implemented: a failed `unlink` would retire a path that is still
  there, and that direction drops a real input.
- D7's reduced-fidelity seccomp path and D8's unexplained-mutation guard both
  become moot.

**Why this is the better design, not merely a constraint.** The Linux seccomp
supervisor responds with `SECCOMP_USER_NOTIF_FLAG_CONTINUE` and is notified
before the syscall, so it can never observe an outcome. Any outcome-dependent
rule would have silently degraded there while working on macOS, Windows and the
Linux preload — a correctness rule that holds on three backends and not the
fourth. Restricting the rules to pre-call information gives all four identical
behaviour and removes the asymmetry entirely.

It also makes a trace a function of the call arguments alone rather than of
filesystem state at trace time, so two runs over the same inputs record the same
events.

**What still works, using only pre-call information:**

- intent from `O_ACCMODE`, plus `O_CREAT`, `O_TRUNC` and `O_EXCL`
- mutation defined as `O_TRUNC`, or `O_CREAT | O_EXCL`, or being a rename
  destination
- write-first versus read-first ordering
- the gitignore tie-break, which is not access data at all
- rename pairs and directory-rename expansion, recorded as attempts
- ignoring bare directory stats

**Two consequences accepted.** A failed probe now counts as a read, so a tool
that checks for a generated file before creating it looks like it read it first;
for ignored paths the gitignore clause still resolves that to an output, and for
tracked paths the run is conservatively not cached. And a failed rename
over-collects its destination, which rule 7 permits.

**Not an outcome.** Checking whether a path exists at archive time is a
filesystem query at decision time, not a syscall outcome, and it stays. It is
also what filters the over-collection above.

## D10 — Verified against the real emdash workload before touching Windows

Reordered on request to de-risk the chain: prove the goal on macOS with a local
`[patch]` override before investing in the Windows backend.

**Setup.** A vite-plus worktree at
`~/.local/share/opencode/worktree/vite-plus-autotrack` with the local-development
patch section enabled, pointing every vite-task crate at this worktree.

**Two traps found while doing it, both worth remembering.**

The `vp` binary does not link `vite_task` at all — only `packages/cli/binding`,
the napi addon, does. So `vp run` is JavaScript calling a native addon, and every
early emdash run measured released vite-plus 0.2.2 rather than this branch.
Swapping a locally built `.node` into emdash's store does not work either: the
addon needs `--features rolldown`, and a 0.2.6 addon against a 0.2.2 JS CLI
loads but produces no output. Verification therefore ran through `vt`, vite-task's
own CLI, which links the crates directly.

Separately, three artifacts must be rebuilt for a change to take effect, and each
one silently reported stale results when missed: the preload dylib consumed by
`fspy --example cli`, the `vt` binary, and the napi addon.

**Result on `@emdash-cms/auth`, a plain `tsdown` build, with no manual globs:**

- run 1 cold: builds, 28 outputs collected, 3074 inputs
- run 2 unchanged: cache hit
- run 3 after `rm -rf dist`: cache hit **and** all 28 files restored
- restored tree is byte-for-byte identical to a fresh build, verified by shasum
- **zero paths under the package's own `dist/` appear in the input set**

That last point is the failure mode that matters: if outputs were fingerprinted
as inputs, run 3 would miss forever on a clean checkout.

## D11 — emdash's full build is blocked by its own environment, not by tracking

`@emdash-cms/registry-lexicons#build` runs `pnpm run build:lexicons` and fails
under both this branch and released vite-plus. Released vite-plus reports the
real cause: `ERR_PNPM_VERIFY_DEPS_BEFORE_RUN`, the workspace structure changed
since the last install. Under `vt` the same failure surfaces less legibly, as a
pnpm SEA assertion (`(magic) == (kMagic)`) because pnpm is a single-executable
binary re-reading its own embedded blob.

Not a tracking defect. emdash needs `pnpm install` before the full build can be
measured.

## D12 — pnpm's single-executable binary fails under fspy; pre-existing

`@emdash-cms/registry-lexicons#build` shells out to `pnpm run build:lexicons` and
aborts with `Assertion failed: (magic) == (kMagic)` inside
`node::sea::FindSingleExecutableResource`. pnpm 11.9.0 ships as a Node
single-executable application, so it re-reads its own binary to find an embedded
blob, and that read does not survive tracing.

**Verified pre-existing**, not caused by this branch: a `vt` built from the base
commit `b3ebf564` reproduces the identical assertion. Released vite-plus does not
hit it because it supplies a managed Node and pnpm rather than the mise shim.

Left alone. It is orthogonal to auto-tracking, blocks only the one package whose
build re-enters pnpm, and fixing it means changing how fspy handles a process
reading its own executable. Verification of the tracking rules therefore excludes
that package.

## D13 — A task's own derived output tree is not an input

Found by running emdash's full build, not by reasoning. With the rules as first
written, 22 of 23 tasks cached and `@emdash-cms/admin#build`'s `tsdown` segment
missed forever with `'messages.mjs' added in 'packages/admin/dist/locales/fa'`.

**Cause.** admin's build is a compound command, and each `&&` segment is a
separate cached task:

```
node --run locale:compile && tsdown && node --run locale:copy && npx tailwindcss
```

`tsdown` _reads_ `dist/locales/<locale>/messages.mjs`, and the later
`locale:copy` segment rewrites those files. So tsdown fingerprints content that a
sibling task changes afterwards, and the fingerprint can never settle.

**Why the existing rules could not catch it.** The "listed a directory it wrote
into" rule does not fire, because tsdown did not write there — a sibling did.
Rejecting on gitignore alone is not available either: a package reading
`node_modules/<dep>/dist/index.mjs` is also reading ignored, derived content, and
that must stay a real input or changing a dependency would stop invalidating its
consumers. That is the central monorepo relationship.

**Rule.** A read path is the task's own derived state when **both** hold: version
control calls it derived, **and** it sits under a directory this same task wrote
into. Requiring both distinguishes the two cases exactly — admin writes
`dist/index.mjs`, so `packages/admin/dist` is one of its own write subtrees and
everything ignored beneath it is its own output tree, while no package writes into
`node_modules/<dep>/dist`, so dependency outputs stay inputs.

Directory listings follow the same principle: a listing is rejected if the task
wrote into that directory or if version control calls the directory derived.

**Result on emdash, zero manual globs:**

- 23/23 cache hit on the second run, stable across repeats
- after deleting every `packages/*/dist`, 22/23 hit and all 1358 files restored
  **byte-for-byte identical**
- the single miss is correct: deleting `packages/core/dist` also empties
  `node_modules/emdash/dist`, which is a genuine input of another task, so it
  rebuilt
