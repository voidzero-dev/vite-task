/**
 * Tell the runner to ignore reads under `path` when inferring cache inputs.
 *
 * No-op when not running inside a runner.
 *
 * @param {string} path
 * @returns {void}
 */
export function ignoreInput(path: string): void;
/**
 * Tell the runner to ignore writes under `path` when inferring cache outputs.
 *
 * No-op when not running inside a runner.
 *
 * @param {string} path
 * @returns {void}
 */
export function ignoreOutput(path: string): void;
/**
 * Tell the runner not to cache this run.
 *
 * No-op when not running inside a runner.
 *
 * @returns {void}
 */
export function disableCache(): void;
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
export function getEnv(name: string, options?: {
    tracked?: boolean;
}): string | undefined;
/**
 * Ask the runner for every env whose name matches `pattern` (a glob, e.g.
 * `VITE_*`) and return the match-set as a plain object.
 *
 * With `tracked: true` (the default) the runner records the pattern as a
 * dependency, so adding, removing, or changing a matching env invalidates
 * this run's cache entry.
 *
 * Has no effect on `process.env`; the caller decides what to do with the
 * returned values. Returns an empty object when not running inside a runner.
 *
 * @param {string} pattern
 * @param {{ tracked?: boolean }} [options]
 * @returns {Record<string, string>}
 */
export function getEnvs(pattern: string, options?: {
    tracked?: boolean;
}): Record<string, string>;
