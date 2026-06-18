# task graph

```mermaid
flowchart TD
  task_0["<workspace>/packages/app#test_dependencies"]
  task_1["<workspace>/packages/app#test_dev"]
  task_2["<workspace>/packages/app#test_peer"]
  task_3["<workspace>/packages/app#test_recursive"]
  task_4["<workspace>/packages/app#test_union"]
  task_5["<workspace>/packages/dev-a#build"]
  task_6["<workspace>/packages/dev-a#build_recursive"]
  task_7["<workspace>/packages/dev-b#build"]
  task_8["<workspace>/packages/dev-b#build_recursive"]
  task_9["<workspace>/packages/nearest-order-app#build"]
  task_10["<workspace>/packages/nearest-order-direct#build"]
  task_11["<workspace>/packages/nearest-order-shared#build"]
  task_12["<workspace>/packages/nearest-order-skip#lint"]
  task_13["<workspace>/packages/nearest-stop-app#build"]
  task_14["<workspace>/packages/nearest-stop-bar#build"]
  task_15["<workspace>/packages/nearest-stop-foo#build"]
  task_16["<workspace>/packages/nearest-through-app#build"]
  task_17["<workspace>/packages/nearest-through-bar#build"]
  task_18["<workspace>/packages/nearest-through-foo#lint"]
  task_19["<workspace>/packages/peer-a#build"]
  task_20["<workspace>/packages/peer-b#build"]
  task_21["<workspace>/packages/prod-a#build"]
  task_22["<workspace>/packages/prod-b#build"]
  task_23["<workspace>/packages/shared#build"]
  task_24["<workspace>/packages/shared#build_recursive"]
```

## `<workspace>/packages/app#test_dependencies`

```json
{
  "task_display": {
    "package_name": "@test/app",
    "task_name": "test_dependencies",
    "package_path": "<workspace>/packages/app"
  },
  "resolved_config": {
    "commands": [
      "vtt test dependencies"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/app",
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

## `<workspace>/packages/app#test_dev`

```json
{
  "task_display": {
    "package_name": "@test/app",
    "task_name": "test_dev",
    "package_path": "<workspace>/packages/app"
  },
  "resolved_config": {
    "commands": [
      "vtt test dev"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/app",
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

## `<workspace>/packages/app#test_peer`

```json
{
  "task_display": {
    "package_name": "@test/app",
    "task_name": "test_peer",
    "package_path": "<workspace>/packages/app"
  },
  "resolved_config": {
    "commands": [
      "vtt test peer"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/app",
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

## `<workspace>/packages/app#test_recursive`

```json
{
  "task_display": {
    "package_name": "@test/app",
    "task_name": "test_recursive",
    "package_path": "<workspace>/packages/app"
  },
  "resolved_config": {
    "commands": [
      "vtt test recursive"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/app",
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

## `<workspace>/packages/app#test_union`

```json
{
  "task_display": {
    "package_name": "@test/app",
    "task_name": "test_union",
    "package_path": "<workspace>/packages/app"
  },
  "resolved_config": {
    "commands": [
      "vtt test union"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/app",
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

## `<workspace>/packages/dev-a#build`

```json
{
  "task_display": {
    "package_name": "@test/dev-a",
    "task_name": "build",
    "package_path": "<workspace>/packages/dev-a"
  },
  "resolved_config": {
    "commands": [
      "vtt build dev-a"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/dev-a",
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

## `<workspace>/packages/dev-a#build_recursive`

```json
{
  "task_display": {
    "package_name": "@test/dev-a",
    "task_name": "build_recursive",
    "package_path": "<workspace>/packages/dev-a"
  },
  "resolved_config": {
    "commands": [
      "vtt build recursive dev-a"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/dev-a",
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

## `<workspace>/packages/dev-b#build`

```json
{
  "task_display": {
    "package_name": "@test/dev-b",
    "task_name": "build",
    "package_path": "<workspace>/packages/dev-b"
  },
  "resolved_config": {
    "commands": [
      "vtt build dev-b"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/dev-b",
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

## `<workspace>/packages/dev-b#build_recursive`

```json
{
  "task_display": {
    "package_name": "@test/dev-b",
    "task_name": "build_recursive",
    "package_path": "<workspace>/packages/dev-b"
  },
  "resolved_config": {
    "commands": [
      "vtt build recursive dev-b"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/dev-b",
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

## `<workspace>/packages/nearest-order-app#build`

```json
{
  "task_display": {
    "package_name": "@test/nearest-order-app",
    "task_name": "build",
    "package_path": "<workspace>/packages/nearest-order-app"
  },
  "resolved_config": {
    "commands": [
      "vtt build nearest-order-app"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/nearest-order-app",
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

## `<workspace>/packages/nearest-order-direct#build`

```json
{
  "task_display": {
    "package_name": "@test/nearest-order-direct",
    "task_name": "build",
    "package_path": "<workspace>/packages/nearest-order-direct"
  },
  "resolved_config": {
    "commands": [
      "vtt build nearest-order-direct"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/nearest-order-direct",
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

## `<workspace>/packages/nearest-order-shared#build`

```json
{
  "task_display": {
    "package_name": "@test/nearest-order-shared",
    "task_name": "build",
    "package_path": "<workspace>/packages/nearest-order-shared"
  },
  "resolved_config": {
    "commands": [
      "vtt build nearest-order-shared"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/nearest-order-shared",
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

## `<workspace>/packages/nearest-order-skip#lint`

```json
{
  "task_display": {
    "package_name": "@test/nearest-order-skip",
    "task_name": "lint",
    "package_path": "<workspace>/packages/nearest-order-skip"
  },
  "resolved_config": {
    "commands": [
      "vtt lint nearest-order-skip"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/nearest-order-skip",
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

## `<workspace>/packages/nearest-stop-app#build`

```json
{
  "task_display": {
    "package_name": "@test/nearest-stop-app",
    "task_name": "build",
    "package_path": "<workspace>/packages/nearest-stop-app"
  },
  "resolved_config": {
    "commands": [
      "vtt build nearest-stop-app"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/nearest-stop-app",
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

## `<workspace>/packages/nearest-stop-bar#build`

```json
{
  "task_display": {
    "package_name": "@test/nearest-stop-bar",
    "task_name": "build",
    "package_path": "<workspace>/packages/nearest-stop-bar"
  },
  "resolved_config": {
    "commands": [
      "vtt build nearest-stop-bar"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/nearest-stop-bar",
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

## `<workspace>/packages/nearest-stop-foo#build`

```json
{
  "task_display": {
    "package_name": "@test/nearest-stop-foo",
    "task_name": "build",
    "package_path": "<workspace>/packages/nearest-stop-foo"
  },
  "resolved_config": {
    "commands": [
      "vtt build nearest-stop-foo"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/nearest-stop-foo",
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

## `<workspace>/packages/nearest-through-app#build`

```json
{
  "task_display": {
    "package_name": "@test/nearest-through-app",
    "task_name": "build",
    "package_path": "<workspace>/packages/nearest-through-app"
  },
  "resolved_config": {
    "commands": [
      "vtt build nearest-through-app"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/nearest-through-app",
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

## `<workspace>/packages/nearest-through-bar#build`

```json
{
  "task_display": {
    "package_name": "@test/nearest-through-bar",
    "task_name": "build",
    "package_path": "<workspace>/packages/nearest-through-bar"
  },
  "resolved_config": {
    "commands": [
      "vtt build nearest-through-bar"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/nearest-through-bar",
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

## `<workspace>/packages/nearest-through-foo#lint`

```json
{
  "task_display": {
    "package_name": "@test/nearest-through-foo",
    "task_name": "lint",
    "package_path": "<workspace>/packages/nearest-through-foo"
  },
  "resolved_config": {
    "commands": [
      "vtt lint nearest-through-foo"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/nearest-through-foo",
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

## `<workspace>/packages/peer-a#build`

```json
{
  "task_display": {
    "package_name": "@test/peer-a",
    "task_name": "build",
    "package_path": "<workspace>/packages/peer-a"
  },
  "resolved_config": {
    "commands": [
      "vtt build peer-a"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/peer-a",
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

## `<workspace>/packages/peer-b#build`

```json
{
  "task_display": {
    "package_name": "@test/peer-b",
    "task_name": "build",
    "package_path": "<workspace>/packages/peer-b"
  },
  "resolved_config": {
    "commands": [
      "vtt build peer-b"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/peer-b",
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

## `<workspace>/packages/prod-a#build`

```json
{
  "task_display": {
    "package_name": "@test/prod-a",
    "task_name": "build",
    "package_path": "<workspace>/packages/prod-a"
  },
  "resolved_config": {
    "commands": [
      "vtt build prod-a"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/prod-a",
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

## `<workspace>/packages/prod-b#build`

```json
{
  "task_display": {
    "package_name": "@test/prod-b",
    "task_name": "build",
    "package_path": "<workspace>/packages/prod-b"
  },
  "resolved_config": {
    "commands": [
      "vtt build prod-b"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/prod-b",
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

## `<workspace>/packages/shared#build`

```json
{
  "task_display": {
    "package_name": "@test/shared",
    "task_name": "build",
    "package_path": "<workspace>/packages/shared"
  },
  "resolved_config": {
    "commands": [
      "vtt build shared"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/shared",
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

## `<workspace>/packages/shared#build_recursive`

```json
{
  "task_display": {
    "package_name": "@test/shared",
    "task_name": "build_recursive",
    "package_path": "<workspace>/packages/shared"
  },
  "resolved_config": {
    "commands": [
      "vtt build recursive shared"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/shared",
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

