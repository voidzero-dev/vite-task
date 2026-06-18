# task graph

```mermaid
flowchart TD
  task_0["<workspace>/packages/api#build"]
  task_1["<workspace>/packages/api#dev"]
  task_2["<workspace>/packages/api#start"]
  task_3["<workspace>/packages/api#test"]
  task_4["<workspace>/packages/app#build"]
  task_5["<workspace>/packages/app#deploy"]
  task_6["<workspace>/packages/app#dev"]
  task_7["<workspace>/packages/app#preview"]
  task_8["<workspace>/packages/app#test"]
  task_9["<workspace>/packages/config#build"]
  task_10["<workspace>/packages/config#validate"]
  task_11["<workspace>/packages/pkg#special#build"]
  task_12["<workspace>/packages/pkg#special#test"]
  task_13["<workspace>/packages/shared#build"]
  task_14["<workspace>/packages/shared#lint"]
  task_15["<workspace>/packages/shared#test"]
  task_16["<workspace>/packages/shared#typecheck"]
  task_17["<workspace>/packages/tools#generate"]
  task_18["<workspace>/packages/tools#validate"]
  task_19["<workspace>/packages/ui#build"]
  task_20["<workspace>/packages/ui#lint"]
  task_21["<workspace>/packages/ui#storybook"]
  task_22["<workspace>/packages/ui#test"]
```

## `<workspace>/packages/api#build`

```json
{
  "task_display": {
    "package_name": "@test/api",
    "task_name": "build",
    "package_path": "<workspace>/packages/api"
  },
  "resolved_config": {
    "commands": [
      "echo Generate schemas && echo Compile TypeScript && echo Bundle API && echo Copy assets"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/api",
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

## `<workspace>/packages/api#dev`

```json
{
  "task_display": {
    "package_name": "@test/api",
    "task_name": "dev",
    "package_path": "<workspace>/packages/api"
  },
  "resolved_config": {
    "commands": [
      "echo Watch mode && echo Start dev server"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/api",
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

## `<workspace>/packages/api#start`

```json
{
  "task_display": {
    "package_name": "@test/api",
    "task_name": "start",
    "package_path": "<workspace>/packages/api"
  },
  "resolved_config": {
    "commands": [
      "echo Starting API server"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/api",
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

## `<workspace>/packages/api#test`

```json
{
  "task_display": {
    "package_name": "@test/api",
    "task_name": "test",
    "package_path": "<workspace>/packages/api"
  },
  "resolved_config": {
    "commands": [
      "echo Testing API"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/api",
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
      "echo Clean dist && echo Build client && echo Build server && echo Generate manifest && echo Optimize assets"
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
      "echo Validate && echo Upload && echo Verify"
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

## `<workspace>/packages/app#dev`

```json
{
  "task_display": {
    "package_name": "@test/app",
    "task_name": "dev",
    "package_path": "<workspace>/packages/app"
  },
  "resolved_config": {
    "commands": [
      "echo Running dev server"
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

## `<workspace>/packages/app#preview`

```json
{
  "task_display": {
    "package_name": "@test/app",
    "task_name": "preview",
    "package_path": "<workspace>/packages/app"
  },
  "resolved_config": {
    "commands": [
      "echo Preview build"
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
      "echo Unit tests && echo Integration tests"
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

## `<workspace>/packages/config#build`

```json
{
  "task_display": {
    "package_name": "@test/config",
    "task_name": "build",
    "package_path": "<workspace>/packages/config"
  },
  "resolved_config": {
    "commands": [
      "echo Building config"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/config",
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

## `<workspace>/packages/config#validate`

```json
{
  "task_display": {
    "package_name": "@test/config",
    "task_name": "validate",
    "package_path": "<workspace>/packages/config"
  },
  "resolved_config": {
    "commands": [
      "echo Validating config"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/config",
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

## `<workspace>/packages/pkg#special#build`

```json
{
  "task_display": {
    "package_name": "@test/pkg#special",
    "task_name": "build",
    "package_path": "<workspace>/packages/pkg#special"
  },
  "resolved_config": {
    "commands": [
      "echo Building package with hash"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/pkg#special",
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

## `<workspace>/packages/pkg#special#test`

```json
{
  "task_display": {
    "package_name": "@test/pkg#special",
    "task_name": "test",
    "package_path": "<workspace>/packages/pkg#special"
  },
  "resolved_config": {
    "commands": [
      "echo Testing package with hash"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/pkg#special",
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
      "echo Cleaning && echo Compiling shared && echo Generating types"
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

## `<workspace>/packages/shared#lint`

```json
{
  "task_display": {
    "package_name": "@test/shared",
    "task_name": "lint",
    "package_path": "<workspace>/packages/shared"
  },
  "resolved_config": {
    "commands": [
      "echo Linting shared"
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

## `<workspace>/packages/shared#test`

```json
{
  "task_display": {
    "package_name": "@test/shared",
    "task_name": "test",
    "package_path": "<workspace>/packages/shared"
  },
  "resolved_config": {
    "commands": [
      "echo Setting up test env && echo Running tests && echo Cleanup"
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

## `<workspace>/packages/shared#typecheck`

```json
{
  "task_display": {
    "package_name": "@test/shared",
    "task_name": "typecheck",
    "package_path": "<workspace>/packages/shared"
  },
  "resolved_config": {
    "commands": [
      "echo Type checking shared"
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

## `<workspace>/packages/tools#generate`

```json
{
  "task_display": {
    "package_name": "@test/tools",
    "task_name": "generate",
    "package_path": "<workspace>/packages/tools"
  },
  "resolved_config": {
    "commands": [
      "echo Generating tools"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/tools",
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

## `<workspace>/packages/tools#validate`

```json
{
  "task_display": {
    "package_name": "@test/tools",
    "task_name": "validate",
    "package_path": "<workspace>/packages/tools"
  },
  "resolved_config": {
    "commands": [
      "echo Validating"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/tools",
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

## `<workspace>/packages/ui#build`

```json
{
  "task_display": {
    "package_name": "@test/ui",
    "task_name": "build",
    "package_path": "<workspace>/packages/ui"
  },
  "resolved_config": {
    "commands": [
      "echo Compile styles && echo Build components && echo Generate types"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/ui",
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

## `<workspace>/packages/ui#lint`

```json
{
  "task_display": {
    "package_name": "@test/ui",
    "task_name": "lint",
    "package_path": "<workspace>/packages/ui"
  },
  "resolved_config": {
    "commands": [
      "echo Linting UI"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/ui",
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

## `<workspace>/packages/ui#storybook`

```json
{
  "task_display": {
    "package_name": "@test/ui",
    "task_name": "storybook",
    "package_path": "<workspace>/packages/ui"
  },
  "resolved_config": {
    "commands": [
      "echo Running storybook"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/ui",
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

## `<workspace>/packages/ui#test`

```json
{
  "task_display": {
    "package_name": "@test/ui",
    "task_name": "test",
    "package_path": "<workspace>/packages/ui"
  },
  "resolved_config": {
    "commands": [
      "echo Testing UI"
    ],
    "resolved_options": {
      "cwd": "<workspace>/packages/ui",
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

