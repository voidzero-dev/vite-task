# task graph

```mermaid
flowchart TD
  task_0["<workspace>/packages/a#from_dependencies"]
  task_0 --> task_2
  task_1["<workspace>/packages/a#from_dev_dependencies"]
  task_1 --> task_2
  task_2["<workspace>/packages/b#build"]
```

## `<workspace>/packages/a#from_dependencies`

```json
{
  "task_display": {
    "package_name": "@test/a",
    "task_name": "from_dependencies",
    "package_path": "<workspace>/packages/a"
  },
  "resolved_config": {
    "commands": [
      "vtt from dependencies"
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
  "source": "TaskConfig"
}
```

## `<workspace>/packages/a#from_dev_dependencies`

```json
{
  "task_display": {
    "package_name": "@test/a",
    "task_name": "from_dev_dependencies",
    "package_path": "<workspace>/packages/a"
  },
  "resolved_config": {
    "commands": [
      "vtt from devDependencies"
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
  "source": "TaskConfig"
}
```

## `<workspace>/packages/b#build`

```json
{
  "task_display": {
    "package_name": "@test/b",
    "task_name": "build",
    "package_path": "<workspace>/packages/b"
  },
  "resolved_config": {
    "commands": [
      "vtt build b"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/b",
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

