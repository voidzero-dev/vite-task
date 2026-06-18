# task graph

```mermaid
flowchart TD
  task_0["<workspace>/#build"]
  task_1["<workspace>/#lint"]
  task_2["<workspace>/#lint-no-cache"]
  task_3["<workspace>/#lint-with-cache"]
  task_4["<workspace>/#lint-with-untracked-env"]
  task_5["<workspace>/#run-build-cache-false"]
  task_6["<workspace>/#run-build-no-cache"]
```

## `<workspace>/#build`

```json
{
  "task_display": {
    "package_name": "@test/synthetic-cache-disabled",
    "task_name": "build",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "vt tool print lint"
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

## `<workspace>/#lint`

```json
{
  "task_display": {
    "package_name": "@test/synthetic-cache-disabled",
    "task_name": "lint",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "vt tool print lint"
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

## `<workspace>/#lint-no-cache`

```json
{
  "task_display": {
    "package_name": "@test/synthetic-cache-disabled",
    "task_name": "lint-no-cache",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "vt tool print lint"
    ],
    "resolved_options": {
      "cwd": "<workspace>/",
      "cache_config": null
    }
  },
  "source": "TaskConfig"
}
```

## `<workspace>/#lint-with-cache`

```json
{
  "task_display": {
    "package_name": "@test/synthetic-cache-disabled",
    "task_name": "lint-with-cache",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "vt tool print lint"
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

## `<workspace>/#lint-with-untracked-env`

```json
{
  "task_display": {
    "package_name": "@test/synthetic-cache-disabled",
    "task_name": "lint-with-untracked-env",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "vt tool print lint"
    ],
    "resolved_options": {
      "cwd": "<workspace>/",
      "cache_config": {
        "env_config": {
          "fingerprinted_envs": [],
          "untracked_env": [
            "CUSTOM_VAR",
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

## `<workspace>/#run-build-cache-false`

```json
{
  "task_display": {
    "package_name": "@test/synthetic-cache-disabled",
    "task_name": "run-build-cache-false",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "vt run build"
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

## `<workspace>/#run-build-no-cache`

```json
{
  "task_display": {
    "package_name": "@test/synthetic-cache-disabled",
    "task_name": "run-build-no-cache",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "vt run build"
    ],
    "resolved_options": {
      "cwd": "<workspace>/",
      "cache_config": null
    }
  },
  "source": "TaskConfig"
}
```

