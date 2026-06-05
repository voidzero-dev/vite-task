# vite_glob

Centralizes glob-matching semantics so every crate in the workspace matches
patterns the same way, instead of each call site reaching for an ad-hoc glob
engine with subtly different rules (separators, case sensitivity, negation).

Two use cases, each with its own module, matcher, and error type:

- **`env`** — environment-variable **name** matching. Names are flat strings,
  not paths, so this is backed by `globset` with path-separator handling
  disabled: `*`/`?`/`[...]`/`{a,b}` are plain-string wildcards, and matching is
  case-sensitive on Unix and case-insensitive on Windows (mirroring env lookup).
  Use `EnvGlob` for one literal pattern, or `EnvGlobSet` for a set with
  negation: a `!`-prefixed pattern excludes (e.g. `["VITE_*", "!VITE_SECRET"]`).
- **`path`** — filesystem **path** matching with gitignore semantics, backed by
  `wax`. `!`-prefixed patterns negate; first-match-wins, or last-match-wins once
  any negation is present. Use `PathGlobSet`.

Keeping both behind one crate means a change to how, say, env names are matched
happens in exactly one place and applies everywhere — the runner's cache
fingerprinting, the IPC server's `getEnvs`, workspace package discovery, and so
on.
