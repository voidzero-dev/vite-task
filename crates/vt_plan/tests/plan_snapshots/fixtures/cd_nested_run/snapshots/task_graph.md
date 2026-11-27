# task graph

```mermaid
flowchart TD
  task_0["<workspace>/#hello"]
  task_1["<workspace>/#plain"]
```

## `<workspace>/#hello`

```json
{
  "task_display": {
    "package_name": "root",
    "task_name": "hello",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "echo ROOT"
    ],
    "resolved_options": {
      "cwd": "<workspace>/",
      "cache_config": {
        "remote_cache_allowed": true,
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

## `<workspace>/#plain`

```json
{
  "task_display": {
    "package_name": "root",
    "task_name": "plain",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "cd ui && vpr hello"
    ],
    "resolved_options": {
      "cwd": "<workspace>/",
      "cache_config": {
        "remote_cache_allowed": true,
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

