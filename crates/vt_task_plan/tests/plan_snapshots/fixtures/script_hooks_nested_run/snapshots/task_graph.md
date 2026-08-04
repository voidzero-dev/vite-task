# task graph

```mermaid
flowchart TD
  task_0["<workspace>/#prescriptInHook"]
  task_1["<workspace>/#pretest"]
  task_2["<workspace>/#scriptInHook"]
  task_3["<workspace>/#test"]
```

## `<workspace>/#prescriptInHook`

```json
{
  "task_display": {
    "package_name": "@test/script-hooks-nested-run",
    "task_name": "prescriptInHook",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "echo prescriptInHook"
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

## `<workspace>/#pretest`

```json
{
  "task_display": {
    "package_name": "@test/script-hooks-nested-run",
    "task_name": "pretest",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "vt run scriptInHook"
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

## `<workspace>/#scriptInHook`

```json
{
  "task_display": {
    "package_name": "@test/script-hooks-nested-run",
    "task_name": "scriptInHook",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "echo scriptInHook"
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

## `<workspace>/#test`

```json
{
  "task_display": {
    "package_name": "@test/script-hooks-nested-run",
    "task_name": "test",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "echo test"
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

