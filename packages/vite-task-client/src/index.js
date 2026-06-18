// The JSDoc in this file is the source of truth for the package's public
// types. `index.d.ts` is generated from it via `pnpm run build:types`
// (using `tsc` with `@tsconfig/strictest`) — edit JSDoc here, not the
// `.d.ts`. CI fails if the committed `.d.ts` drifts from a fresh regen.

import { createRequire } from 'node:module';

/**
 * @typedef {{ tracked?: boolean }} GetEnvOptions
 */

/**
 * @typedef {string | { prefix: string }} GetEnvsQuery
 */

/**
 * Methods exposed by the napi addon. Keep this shape in sync with the
 * `RunnerClient` returned by `load()` in
 * `crates/vite_task_client_napi/src/lib.rs` — any new method added there
 * needs a matching entry here, and vice versa.
 *
 * @type {{
 *   ignoreInput: (path: string) => void,
 *   ignoreOutput: (path: string) => void,
 *   disableCache: () => void,
 *   getEnv: (name: string, options?: GetEnvOptions) => string | undefined,
 *   getEnvs: (query: GetEnvsQuery, options?: GetEnvOptions) => Record<string, string>,
 * } | null | undefined}
 */
let addon;

function load() {
  if (addon !== undefined) return addon;
  try {
    const path = process.env['VP_RUN_NODE_CLIENT_PATH'];
    if (path) {
      // The addon exports a `load(options?)` factory rather than the
      // methods directly, so the addon shape can evolve in lockstep with
      // this wrapper: a future wrapper can pass `{ version: N }` to opt
      // into a new shape without breaking older addons that only know v1.
      // Today's wrapper passes nothing and accepts whatever the addon's
      // current default version returns.
      addon = createRequire(import.meta.url)(path).load();
      return addon;
    }
  } catch {
    // Fall through — the runner's IPC env is absent or the addon refused to
    // load. Memoize the unavailable decision so subsequent calls don't retry.
  }
  addon = null;
  return addon;
}

/**
 * Tell the runner to ignore reads under `path` when inferring cache inputs.
 *
 * No-op when not running inside a runner.
 *
 * @param {string} path
 * @returns {void}
 */
export function ignoreInput(path) {
  load()?.ignoreInput(path);
}

/**
 * Tell the runner to ignore writes under `path` when inferring cache outputs.
 *
 * No-op when not running inside a runner.
 *
 * @param {string} path
 * @returns {void}
 */
export function ignoreOutput(path) {
  load()?.ignoreOutput(path);
}

/**
 * Tell the runner not to cache this run.
 *
 * No-op when not running inside a runner.
 *
 * @returns {void}
 */
export function disableCache() {
  load()?.disableCache();
}

/**
 * Ask the runner for the value of the env var `name` and return it, or
 * `undefined` when the runner has no such env.
 *
 * With `tracked: true` (the default) the runner records `name` as a
 * dependency, so a change to its value invalidates this run's cache entry.
 *
 * Has no effect on `process.env`; the caller decides what to do with the
 * returned value. Returns `undefined` when not running inside a runner.
 *
 * @param {string} name
 * @param {{ tracked?: boolean }} [options]
 * @returns {string | undefined}
 */
export function getEnv(name, options) {
  const a = load();
  if (!a) return undefined;
  return a.getEnv(name, options);
}

/**
 * Ask the runner for matching envs and return the match-set as a plain object.
 *
 * Pass a glob string (e.g. `VITE_*`) to use glob matching, or pass
 * `{ prefix: 'VITE_' }` to match env names by literal prefix.
 *
 * With `tracked: true` (the default) the runner records the pattern as a
 * dependency, so adding, removing, or changing a matching env invalidates
 * this run's cache entry.
 *
 * Has no effect on `process.env`; the caller decides what to do with the
 * returned values. Returns an empty object when not running inside a runner.
 *
 * @param {GetEnvsQuery} query
 * @param {GetEnvOptions} [options]
 * @returns {Record<string, string>}
 */
export function getEnvs(query, options) {
  const a = load();
  if (!a) return {};
  return a.getEnvs(query, options);
}
