# task graph

```mermaid
flowchart TD
  task_0["<workspace>/#check"]
  task_1["<workspace>/packages/app#build"]
  task_2["<workspace>/packages/app#check"]
  task_3["<workspace>/packages/app#deploy"]
  task_4["<workspace>/packages/app#test"]
  task_5["<workspace>/packages/cli#build"]
  task_6["<workspace>/packages/cli#test"]
  task_7["<workspace>/packages/core#build"]
  task_8["<workspace>/packages/core#check"]
  task_9["<workspace>/packages/core#test"]
  task_10["<workspace>/packages/lib#build"]
  task_11["<workspace>/packages/lib#test"]
  task_12["<workspace>/packages/utils#build"]
```

## `<workspace>/#check`

```json
{
  "task_display": {
    "package_name": "test-workspace",
    "task_name": "check",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "echo 'Checking workspace'"
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

## `<workspace>/packages/app#build`

```json
{
  "task_display": {
    "package_name": "@test/app",
    "task_name": "build",
    "package_path": "<workspace>/packages/app"
  },
  "resolved_config": {
    "commands": [
      "echo 'Building @test/app'"
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
  "source": "PackageJsonScript"
}
```

## `<workspace>/packages/app#check`

```json
{
  "task_display": {
    "package_name": "@test/app",
    "task_name": "check",
    "package_path": "<workspace>/packages/app"
  },
  "resolved_config": {
    "commands": [
      "echo 'Checking @test/app'"
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
  "source": "PackageJsonScript"
}
```

## `<workspace>/packages/app#deploy`

```json
{
  "task_display": {
    "package_name": "@test/app",
    "task_name": "deploy",
    "package_path": "<workspace>/packages/app"
  },
  "resolved_config": {
    "commands": [
      "vt run --filter .... build"
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
  "source": "PackageJsonScript"
}
```

## `<workspace>/packages/app#test`

```json
{
  "task_display": {
    "package_name": "@test/app",
    "task_name": "test",
    "package_path": "<workspace>/packages/app"
  },
  "resolved_config": {
    "commands": [
      "echo 'Testing @test/app'"
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
  "source": "PackageJsonScript"
}
```

## `<workspace>/packages/cli#build`

```json
{
  "task_display": {
    "package_name": "@test/cli",
    "task_name": "build",
    "package_path": "<workspace>/packages/cli"
  },
  "resolved_config": {
    "commands": [
      "echo 'Building @test/cli'"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/cli",
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

## `<workspace>/packages/cli#test`

```json
{
  "task_display": {
    "package_name": "@test/cli",
    "task_name": "test",
    "package_path": "<workspace>/packages/cli"
  },
  "resolved_config": {
    "commands": [
      "echo 'Testing @test/cli'"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/cli",
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

## `<workspace>/packages/core#build`

```json
{
  "task_display": {
    "package_name": "@test/core",
    "task_name": "build",
    "package_path": "<workspace>/packages/core"
  },
  "resolved_config": {
    "commands": [
      "echo 'Building @test/core'"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/core",
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

## `<workspace>/packages/core#check`

```json
{
  "task_display": {
    "package_name": "@test/core",
    "task_name": "check",
    "package_path": "<workspace>/packages/core"
  },
  "resolved_config": {
    "commands": [
      "echo 'Checking @test/core'"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/core",
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

## `<workspace>/packages/core#test`

```json
{
  "task_display": {
    "package_name": "@test/core",
    "task_name": "test",
    "package_path": "<workspace>/packages/core"
  },
  "resolved_config": {
    "commands": [
      "echo 'Testing @test/core'"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/core",
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

## `<workspace>/packages/lib#build`

```json
{
  "task_display": {
    "package_name": "@test/lib",
    "task_name": "build",
    "package_path": "<workspace>/packages/lib"
  },
  "resolved_config": {
    "commands": [
      "echo 'Building @test/lib'"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/lib",
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

## `<workspace>/packages/lib#test`

```json
{
  "task_display": {
    "package_name": "@test/lib",
    "task_name": "test",
    "package_path": "<workspace>/packages/lib"
  },
  "resolved_config": {
    "commands": [
      "echo 'Testing @test/lib'"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/lib",
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

## `<workspace>/packages/utils#build`

```json
{
  "task_display": {
    "package_name": "@test/utils",
    "task_name": "build",
    "package_path": "<workspace>/packages/utils"
  },
  "resolved_config": {
    "commands": [
      "echo 'Building @test/utils'"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/utils",
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

