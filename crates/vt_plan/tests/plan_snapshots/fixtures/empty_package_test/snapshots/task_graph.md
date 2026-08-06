# task graph

```mermaid
flowchart TD
  task_0["<workspace>/packages/another-empty#build"]
  task_0 --> task_2
  task_0 --> task_8
  task_1["<workspace>/packages/another-empty#deploy"]
  task_1 --> task_0
  task_1 --> task_3
  task_2["<workspace>/packages/another-empty#lint"]
  task_3["<workspace>/packages/another-empty#test"]
  task_4["<workspace>/packages/empty-name#build"]
  task_4 --> task_6
  task_5["<workspace>/packages/empty-name#lint"]
  task_6["<workspace>/packages/empty-name#test"]
  task_7["<workspace>/packages/normal-package#build"]
  task_8["<workspace>/packages/normal-package#test"]
```

## `<workspace>/packages/another-empty#build`

```json
{
  "task_display": {
    "package_name": "",
    "task_name": "build",
    "package_path": "<workspace>/packages/another-empty"
  },
  "resolved_config": {
    "commands": [
      "echo 'Building another-empty package'"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/another-empty",
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

## `<workspace>/packages/another-empty#deploy`

```json
{
  "task_display": {
    "package_name": "",
    "task_name": "deploy",
    "package_path": "<workspace>/packages/another-empty"
  },
  "resolved_config": {
    "commands": [
      "echo 'Deploying another-empty package'"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/another-empty",
      "cache_config": null
    }
  },
  "source": "TaskConfig"
}
```

## `<workspace>/packages/another-empty#lint`

```json
{
  "task_display": {
    "package_name": "",
    "task_name": "lint",
    "package_path": "<workspace>/packages/another-empty"
  },
  "resolved_config": {
    "commands": [
      "echo 'Linting another-empty package'"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/another-empty",
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

## `<workspace>/packages/another-empty#test`

```json
{
  "task_display": {
    "package_name": "",
    "task_name": "test",
    "package_path": "<workspace>/packages/another-empty"
  },
  "resolved_config": {
    "commands": [
      "echo 'Testing another-empty package'"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/another-empty",
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

## `<workspace>/packages/empty-name#build`

```json
{
  "task_display": {
    "package_name": "",
    "task_name": "build",
    "package_path": "<workspace>/packages/empty-name"
  },
  "resolved_config": {
    "commands": [
      "echo 'Building empty-name package'"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/empty-name",
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

## `<workspace>/packages/empty-name#lint`

```json
{
  "task_display": {
    "package_name": "",
    "task_name": "lint",
    "package_path": "<workspace>/packages/empty-name"
  },
  "resolved_config": {
    "commands": [
      "echo 'Linting empty-name package'"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/empty-name",
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

## `<workspace>/packages/empty-name#test`

```json
{
  "task_display": {
    "package_name": "",
    "task_name": "test",
    "package_path": "<workspace>/packages/empty-name"
  },
  "resolved_config": {
    "commands": [
      "echo 'Testing empty-name package'"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/empty-name",
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

## `<workspace>/packages/normal-package#build`

```json
{
  "task_display": {
    "package_name": "normal-package",
    "task_name": "build",
    "package_path": "<workspace>/packages/normal-package"
  },
  "resolved_config": {
    "commands": [
      "echo 'Building normal-package'"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/normal-package",
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

## `<workspace>/packages/normal-package#test`

```json
{
  "task_display": {
    "package_name": "normal-package",
    "task_name": "test",
    "package_path": "<workspace>/packages/normal-package"
  },
  "resolved_config": {
    "commands": [
      "echo 'Testing normal-package'"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/normal-package",
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

