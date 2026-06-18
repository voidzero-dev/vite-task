# task graph

```mermaid
flowchart TD
  task_0["<workspace>/#task-a"]
  task_0 --> task_1
  task_1["<workspace>/#task-b"]
  task_1 --> task_0
```

## `<workspace>/#task-a`

```json
{
  "task_display": {
    "package_name": "cycle-dependency-test",
    "task_name": "task-a",
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
  "source": "TaskConfig"
}
```

## `<workspace>/#task-b`

```json
{
  "task_display": {
    "package_name": "cycle-dependency-test",
    "task_name": "task-b",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "echo b"
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

