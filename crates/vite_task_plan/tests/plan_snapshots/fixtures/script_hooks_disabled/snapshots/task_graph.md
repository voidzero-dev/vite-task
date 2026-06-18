# task graph

```mermaid
flowchart TD
  task_0["<workspace>/#posttest"]
  task_1["<workspace>/#pretest"]
  task_2["<workspace>/#test"]
```

## `<workspace>/#posttest`

```json
{
  "task_display": {
    "package_name": "@test/script-hooks-disabled",
    "task_name": "posttest",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "echo posttest"
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
    "package_name": "@test/script-hooks-disabled",
    "task_name": "pretest",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "echo pretest"
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
    "package_name": "@test/script-hooks-disabled",
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

