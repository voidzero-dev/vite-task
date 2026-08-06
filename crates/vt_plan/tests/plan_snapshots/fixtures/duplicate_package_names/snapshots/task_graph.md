# task graph

```mermaid
flowchart TD
  task_0["<workspace>/packages/pkg-a#build"]
  task_1["<workspace>/packages/pkg-b#build"]
```

## `<workspace>/packages/pkg-a#build`

```json
{
  "task_display": {
    "package_name": "@test/duplicate",
    "task_name": "build",
    "package_path": "<workspace>/packages/pkg-a"
  },
  "resolved_config": {
    "commands": [
      "echo build-a"
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

## `<workspace>/packages/pkg-b#build`

```json
{
  "task_display": {
    "package_name": "@test/duplicate",
    "task_name": "build",
    "package_path": "<workspace>/packages/pkg-b"
  },
  "resolved_config": {
    "commands": [
      "echo build-b"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/pkg-b",
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

