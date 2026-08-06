# task graph

```mermaid
flowchart TD
  task_0["<workspace>/#deploy"]
  task_1["<workspace>/packages/pkg-a#build"]
```

## `<workspace>/#deploy`

```json
{
  "task_display": {
    "package_name": "",
    "task_name": "deploy",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "echo deploying workspace"
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
  "source": "TaskConfig"
}
```

## `<workspace>/packages/pkg-a#build`

```json
{
  "task_display": {
    "package_name": "@test/pkg-a",
    "task_name": "build",
    "package_path": "<workspace>/packages/pkg-a"
  },
  "resolved_config": {
    "commands": [
      "echo building pkg-a"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/pkg-a",
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

