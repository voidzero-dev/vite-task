# task graph

```mermaid
flowchart TD
  task_0["<workspace>/#build"]
  task_1["<workspace>/#build-with-concurrency"]
  task_2["<workspace>/packages/a#build"]
  task_3["<workspace>/packages/b#build"]
  task_4["<workspace>/packages/c#build"]
```

## `<workspace>/#build`

```json
{
  "task_display": {
    "package_name": "test-workspace",
    "task_name": "build",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "vt run -r build"
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

## `<workspace>/#build-with-concurrency`

```json
{
  "task_display": {
    "package_name": "test-workspace",
    "task_name": "build-with-concurrency",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "vt run -r --concurrency-limit 5 build"
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

## `<workspace>/packages/a#build`

```json
{
  "task_display": {
    "package_name": "@test/a",
    "task_name": "build",
    "package_path": "<workspace>/packages/a"
  },
  "resolved_config": {
    "commands": [
      "echo building a"
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
      "echo building b"
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

## `<workspace>/packages/c#build`

```json
{
  "task_display": {
    "package_name": "@test/c",
    "task_name": "build",
    "package_path": "<workspace>/packages/c"
  },
  "resolved_config": {
    "commands": [
      "echo building c"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/c",
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

