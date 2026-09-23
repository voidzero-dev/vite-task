# vt_casefold

`Folded<S, R>` wraps a string `S` so that equality, ordering, and hashing use its folded form under the case rule `R`, while the stored string keeps its original spelling. `S` can be owned storage (any `AsRef<OsStr>`, such as `Arc<OsStr>`, `OsString`, or `&str`) or an unsized string such as `OsStr`.

`Folded<OsStr, R>` is the borrowed form: every `Folded<S, R>` borrows as it, so maps keyed by `Folded` can be searched without allocating a key. `Folded::from_ref` views a `&S` as a `&Folded<S, R>`, for example `envs.get(EnvName::from_ref(OsStr::new("PATH")))`.

Case rules implement `CaseRule`:

- `CaseSensitive` compares bytes exactly.
- `AsciiCaseInsensitive` ignores the case of ASCII letters.
- `EnvNameCaseRule` is the rule for environment variable names on the current platform: `AsciiCaseInsensitive` on Windows, `CaseSensitive` elsewhere. `EnvName` is the matching alias.

Because the case rule is a type parameter, Windows behavior can be tested on every platform by using `AsciiCaseInsensitive` directly.

The optional `serde` feature implements `Serialize` and `Deserialize`, which encode a `Folded` value exactly as its storage, keeping the original spelling. Because differently spelled values can be equal, equal values can encode to different bytes. Keep that in mind wherever encoded bytes are compared, such as cache keys.
