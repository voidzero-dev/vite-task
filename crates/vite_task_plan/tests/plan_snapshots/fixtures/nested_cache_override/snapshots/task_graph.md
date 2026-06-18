# task graph

```mermaid
flowchart TD
  task_0["<workspace>/#inner"]
  task_1["<workspace>/#outer-cache"]
  task_2["<workspace>/#outer-inherit"]
  task_3["<workspace>/#outer-no-cache"]
```

## `<workspace>/#inner`

```json
{
  "task_display": {
    "package_name": "@test/nested-cache-override",
    "task_name": "inner",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "vtt print-file package.json"
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

## `<workspace>/#outer-cache`

```json
{
  "task_display": {
    "package_name": "@test/nested-cache-override",
    "task_name": "outer-cache",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "vt run --cache inner"
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

## `<workspace>/#outer-inherit`

```json
{
  "task_display": {
    "package_name": "@test/nested-cache-override",
    "task_name": "outer-inherit",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "vt run inner"
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

## `<workspace>/#outer-no-cache`

```json
{
  "task_display": {
    "package_name": "@test/nested-cache-override",
    "task_name": "outer-no-cache",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "vt run --no-cache inner"
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

