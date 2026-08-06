# task graph

```mermaid
flowchart TD
  task_0["<workspace>/#deploy"]
  task_1["<workspace>/packages/a#deploy"]
```

## `<workspace>/#deploy`

```json
{
  "task_display": {
    "package_name": "test-workspace",
    "task_name": "deploy",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "cd packages/a && vt run deploy"
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

## `<workspace>/packages/a#deploy`

```json
{
  "task_display": {
    "package_name": "@test/a",
    "task_name": "deploy",
    "package_path": "<workspace>/packages/a"
  },
  "resolved_config": {
    "commands": [
      "echo deploying-a"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/a",
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

