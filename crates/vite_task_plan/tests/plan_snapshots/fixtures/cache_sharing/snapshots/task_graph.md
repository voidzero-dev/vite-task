# task graph

```mermaid
flowchart TD
  task_0["<workspace>/#a"]
  task_1["<workspace>/#b"]
  task_2["<workspace>/#c"]
```

## `<workspace>/#a`

```json
{
  "task_display": {
    "package_name": "@test/cache-sharing",
    "task_name": "a",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "echo a"
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

## `<workspace>/#b`

```json
{
  "task_display": {
    "package_name": "@test/cache-sharing",
    "task_name": "b",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "echo a && echo b"
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

## `<workspace>/#c`

```json
{
  "task_display": {
    "package_name": "@test/cache-sharing",
    "task_name": "c",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "echo a && echo b && echo c"
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

