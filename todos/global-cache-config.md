# Global cache config

- Type: `cache: boolean | { scripts: boolean, tasks: boolean }`
- Replaces `cacheScripts` (which should be removed. Don't need backward compatibility)
- Only allowed in the workspace root config
- `tasks: true` will respect the cache configuration in individual tasks.
- `tasks: false` will disable cache for all tasks, even if they have `cache: true` in their own config.
- `scripts: true` will cache the execution of scripts (without corresponding `tasks` entries) in `package.json` files
- The default value is `{ scripts: false, tasks: true }`
- `cache: true` is equivalent to `{ scripts: true, tasks: true }`
- `cache: false` is equivalent to `{ scripts: false, tasks: false }`
- `vp run` has two new flags: `--cache` and `--no-cache` which can override the global cache config for that specific run. `--cache` has the same effect as `cache: true` and `--no-cache` has the same effect as `cache: false`.
