# task graph

```mermaid
flowchart TD
  task_0["<workspace>/#script1"]
  task_1["<workspace>/#script2"]
```

## `<workspace>/#script1`

```json
{
  "task_display": {
    "package_name": "",
    "task_name": "script1",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "echo hello"
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

## `<workspace>/#script2`

```json
{
  "task_display": {
    "package_name": "",
    "task_name": "script2",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "vt run script1"
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

