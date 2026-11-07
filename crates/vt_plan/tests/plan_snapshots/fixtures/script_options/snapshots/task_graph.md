# task graph

```mermaid
flowchart TD
  task_0["<workspace>/#build"]
  task_0 --> task_2
  task_1["<workspace>/#deploy"]
  task_2["<workspace>/#lint"]
  task_3["<workspace>/#postbuild"]
  task_4["<workspace>/#prebuild"]
  task_4 --> task_5
  task_5["<workspace>/#setup"]
  task_6["<workspace>/#where"]
```

## `<workspace>/#build`

```json
{
  "task_display": {
    "package_name": "@test/script-options",
    "task_name": "build",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "vtt print build"
    ],
    "resolved_options": {
      "cwd": "<workspace>/",
      "cache_config": {
        "remote_cache_allowed": true,
        "env_config": {
          "fingerprinted_envs": [
            "APP_ENV"
          ],
          "untracked_env": [
            "<default untracked envs>"
          ]
        },
        "input_config": {
          "includes_auto": false,
          "positive_globs": [
            "src/**"
          ],
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

## `<workspace>/#deploy`

```json
{
  "task_display": {
    "package_name": "@test/script-options",
    "task_name": "deploy",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "vtt print deploy"
    ],
    "resolved_options": {
      "cwd": "<workspace>/",
      "cache_config": null
    }
  },
  "source": "PackageJsonScript"
}
```

## `<workspace>/#lint`

```json
{
  "task_display": {
    "package_name": "@test/script-options",
    "task_name": "lint",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "vtt print lint"
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

## `<workspace>/#postbuild`

```json
{
  "task_display": {
    "package_name": "@test/script-options",
    "task_name": "postbuild",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "vtt print postbuild"
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

## `<workspace>/#prebuild`

```json
{
  "task_display": {
    "package_name": "@test/script-options",
    "task_name": "prebuild",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "vtt print prebuild"
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

## `<workspace>/#setup`

```json
{
  "task_display": {
    "package_name": "@test/script-options",
    "task_name": "setup",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "vtt print setup"
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

## `<workspace>/#where`

```json
{
  "task_display": {
    "package_name": "@test/script-options",
    "task_name": "where",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "vtt print where"
    ],
    "resolved_options": {
      "cwd": "<workspace>/src",
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

