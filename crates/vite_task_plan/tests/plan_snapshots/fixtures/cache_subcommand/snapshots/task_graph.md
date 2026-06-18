# task graph

```mermaid
flowchart TD
  task_0["<workspace>/#clean-cache"]
```

## `<workspace>/#clean-cache`

```json
{
  "task_display": {
    "package_name": "@test/cache-subcommand",
    "task_name": "clean-cache",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "vt cache clean"
    ],
    "resolved_options": {
      "cwd": "<workspace>/",
      "cache_config": {
        "env_config": {
          "fingerprinted_envs": [],
          "untracked_env": [
            "<default untracked envs>"
          ]
        },
        "input_config": {
          "includes_auto": true,
          "positive_globs": [],
          "negative_globs": []
        },
        "output_config": {
          "includes_auto": true,
          "positive_globs": [],
          "negative_globs": []
        }
      }
    }
  },
  "source": "PackageJsonScript"
}
```

