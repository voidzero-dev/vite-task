# Vite & Rolldown Filesystem Operations

File reads and writes relevant to output restoration and cache fingerprinting.
All paths are relative to the package root unless noted.

## Who writes output files

When Vite uses Rolldown as its bundler, the actual chunk/asset writes happen in
Rolldown's Rust core (`bundle.rs`). Vite calls `bundle.write(output)` on the
`RolldownBuild` object; it does not write chunks itself. Rolldown's TypeScript
`build.ts` is only the standalone public API and is bypassed when called from
Vite.

Vite owns only the surrounding operations: emptying the output dir and copying
public assets.

---

## Vite — Output Directory (`build.outDir`, default `dist/`)

| File                                                                                                                    | Line | Operation                        | Description                                                         |
| ----------------------------------------------------------------------------------------------------------------------- | ---- | -------------------------------- | ------------------------------------------------------------------- |
| [prepareOutDir.ts](https://github.com/vitejs/vite/blob/v8.0.8/packages/vite/src/node/plugins/prepareOutDir.ts#L69)      | 69   | read (`readdirSync`)             | `emptyDir(outDir)` — lists then deletes all contents before build   |
| [utils.ts](https://github.com/vitejs/vite/blob/v8.0.8/packages/vite/src/node/utils.ts#L591)                             | 591  | read (`readdirSync`, `statSync`) | `emptyDir()` and `copyDir()` implementations                        |
| [prepareOutDir.ts](https://github.com/vitejs/vite/blob/v8.0.8/packages/vite/src/node/plugins/prepareOutDir.ts#L89)      | 89   | write                            | `copyDir(publicDir, outDir)` — copies public assets into output dir |
| [build.ts](https://github.com/vitejs/vite/blob/v8.0.8/packages/vite/src/node/build.ts#L874)                             | 874  | write (delegates)                | `bundle.write(output)` — hands off to Rolldown Rust core            |
| [license.ts](https://github.com/vitejs/vite/blob/v8.0.8/packages/vite/src/node/plugins/license.ts#L97)                  | 97   | write                            | emits `dist/.vite/license.json` via `emitFile()`                    |
| [license.ts](https://github.com/vitejs/vite/blob/v8.0.8/packages/vite/src/node/plugins/license.ts#L107)                 | 107  | write                            | emits `dist/.vite/license.md` via `emitFile()`                      |
| [ssrManifestPlugin.ts](https://github.com/vitejs/vite/blob/v8.0.8/packages/vite/src/node/ssr/ssrManifestPlugin.ts#L106) | 106  | write                            | emits `dist/.vite/ssr-manifest.json` via `emitFile()`               |

Note: `manifest.json` is emitted by a native Rolldown plugin (`native:manifest`),
not by Vite JS code.

## Vite — Cache Directory (`cacheDir`, default `node_modules/.vite/`)

| File                                                                                                             | Line | Operation                | Description                                                         |
| ---------------------------------------------------------------------------------------------------------------- | ---- | ------------------------ | ------------------------------------------------------------------- |
| [optimizer/index.ts](https://github.com/vitejs/vite/blob/v8.0.8/packages/vite/src/node/optimizer/index.ts#L405)  | 405  | read                     | reads `cacheDir/deps/_metadata.json`                                |
| [optimizer/index.ts](https://github.com/vitejs/vite/blob/v8.0.8/packages/vite/src/node/optimizer/index.ts#L600)  | 600  | read                     | `existsSync(depsCacheDir)` — checks cache presence                  |
| [optimizer/index.ts](https://github.com/vitejs/vite/blob/v8.0.8/packages/vite/src/node/optimizer/index.ts#L1417) | 1417 | read (`readdir`, `stat`) | scans for stale temp dirs older than 24h                            |
| [optimizer/index.ts](https://github.com/vitejs/vite/blob/v8.0.8/packages/vite/src/node/optimizer/index.ts#L531)  | 531  | write                    | writes `package.json` (`"type":"module"`) into processing cache dir |
| [optimizer/index.ts](https://github.com/vitejs/vite/blob/v8.0.8/packages/vite/src/node/optimizer/index.ts#L586)  | 586  | write                    | writes `_metadata.json`                                             |
| [optimizer/index.ts](https://github.com/vitejs/vite/blob/v8.0.8/packages/vite/src/node/optimizer/index.ts#L858)  | 858  | write                    | `bundle.write()` — writes pre-bundled deps to `cacheDir/deps/`      |
| [config.ts](https://github.com/vitejs/vite/blob/v8.0.8/packages/vite/src/node/config.ts#L2542)                   | 2542 | write                    | creates `node_modules/.vite-temp/` for bundled config files         |

---

## Rolldown — TypeScript API (`output.dir`, default `dist/`)

| File                                                                                                                             | Line | Operation         | Description                                                            |
| -------------------------------------------------------------------------------------------------------------------------------- | ---- | ----------------- | ---------------------------------------------------------------------- |
| [output-options.ts](https://github.com/rolldown/rolldown/blob/v1.0.0-rc.15/packages/rolldown/src/options/output-options.ts#L702) | 702  | —                 | `cleanDir?: boolean` option definition                                 |
| [build.ts](https://github.com/rolldown/rolldown/blob/v1.0.0-rc.15/packages/rolldown/src/api/build.ts#L65)                        | 65   | write (delegates) | `build.write(output)` — standalone entry point, delegates to Rust core |

## Rolldown — Rust Core

| File                                                                                                                  | Line | Operation         | Description                                                           |
| --------------------------------------------------------------------------------------------------------------------- | ---- | ----------------- | --------------------------------------------------------------------- |
| [bundle/bundle.rs](https://github.com/rolldown/rolldown/blob/v1.0.0-rc.15/crates/rolldown/src/bundle/bundle.rs#L161)  | 161  | read + delete     | calls `clean_dir(&fs, &dist_dir)` when `clean_dir` option is set      |
| [bundle/bundle.rs](https://github.com/rolldown/rolldown/blob/v1.0.0-rc.15/crates/rolldown/src/bundle/bundle.rs#L175)  | 175  | write             | `fs.create_dir_all(&dist_dir)` — creates output dir                   |
| [bundle/bundle.rs](https://github.com/rolldown/rolldown/blob/v1.0.0-rc.15/crates/rolldown/src/bundle/bundle.rs#L202)  | 202  | write             | `fs.create_dir_all(p)` — creates parent dirs for output chunks        |
| [bundle/bundle.rs](https://github.com/rolldown/rolldown/blob/v1.0.0-rc.15/crates/rolldown/src/bundle/bundle.rs#L209)  | 209  | write             | `fs.write(&dest, chunk.content_as_bytes())` — writes each output file |
| [utils/fs_utils.rs](https://github.com/rolldown/rolldown/blob/v1.0.0-rc.15/crates/rolldown/src/utils/fs_utils.rs#L10) | 10   | —                 | `clean_dir()` function definition                                     |
| [utils/fs_utils.rs](https://github.com/rolldown/rolldown/blob/v1.0.0-rc.15/crates/rolldown/src/utils/fs_utils.rs#L25) | 25   | read (`read_dir`) | lists directory entries to clean                                      |
| [utils/fs_utils.rs](https://github.com/rolldown/rolldown/blob/v1.0.0-rc.15/crates/rolldown/src/utils/fs_utils.rs#L27) | 27   | delete            | `remove_dir_all` for subdirectories                                   |
| [utils/fs_utils.rs](https://github.com/rolldown/rolldown/blob/v1.0.0-rc.15/crates/rolldown/src/utils/fs_utils.rs#L29) | 29   | delete            | `remove_file` for files                                               |

Rolldown has no disk cache.

---

## Where to add `ignoreInputs` / `ignoreOutputs`

A single Vite plugin calling the runner IPC covers all of the above because
Rolldown's Rust code runs as a NAPI addon inside the same Node.js process —
fspy traces syscalls regardless of whether they originate from JS or Rust.

```ts
// Vite plugin (added once to vite.config.ts, no-op when VP_IPC is absent)
buildStart() {
  const ipcPath = process.env.VP_IPC
  if (!ipcPath) return
  const outDir  = this.environment.config.build.outDir  // e.g. "dist"
  const cacheDir = this.environment.config.cacheDir     // e.g. "node_modules/.vite"
  ignoreInputs([outDir, cacheDir])   // suppress reads: emptyDir, clean_dir, dep optimizer
  ignoreOutputs([cacheDir])          // suppress writes: pre-bundled deps, metadata
  // outDir writes are real outputs — do NOT ignore them
}
```

`ignoreInputs(["dist"])` covers:

- Vite `emptyDir` reads (`readdirSync` in `utils.ts:591`)
- Rolldown `clean_dir` reads (`read_dir` in `fs_utils.rs:25`) — same process, same syscalls

`ignoreInputs(["node_modules/.vite"])` covers:

- Dep optimizer `readFile`, `existsSync`, `readdir` reads

`ignoreOutputs(["node_modules/.vite"])` covers:

- Dep optimizer `bundle.write`, `writeFileSync`, `.vite-temp` writes — not real task outputs

### Injection without modifying `vite.config.ts`

Vite has no env-based plugin injection mechanism. Options:

- **`NODE_OPTIONS=--import`**: monkey-patch Vite's `build`/`createServer` before startup — works but fragile across Vite versions, requires Node 20+
- **Explicit plugin in config**: stable, recommended — the plugin is a no-op outside of `vp run`
