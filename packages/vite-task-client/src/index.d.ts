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
export function getEnvs(query: GetEnvsQuery, options?: GetEnvOptions): Record<string, string>;
export type GetEnvOptions = {
    tracked?: boolean;
};
export type GetEnvsQuery = string | {
    prefix: string;
};
