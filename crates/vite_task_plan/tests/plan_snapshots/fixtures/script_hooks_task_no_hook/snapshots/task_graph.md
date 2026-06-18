# task graph

```mermaid
flowchart TD
  task_0["<workspace>/#pretest"]
  task_1["<workspace>/#test"]
```

## `<workspace>/#pretest`

```json
{
  "task_display": {
    "package_name": "@test/script-hooks-task-no-hook",
    "task_name": "pretest",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "echo pretest-script"
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
    "package_name": "@test/script-hooks-task-no-hook",
    "task_name": "test",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "echo test-task"
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

