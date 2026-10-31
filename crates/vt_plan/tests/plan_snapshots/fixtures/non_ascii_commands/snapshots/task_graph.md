# task graph

```mermaid
flowchart TD
  task_0["<workspace>/#greet"]
  task_1["<workspace>/#greet-and-done"]
  task_2["<workspace>/#greet-and-missing"]
  task_3["<workspace>/#greet-long"]
```

## `<workspace>/#greet`

```json
{
  "task_display": {
    "package_name": "non-ascii-commands",
    "task_name": "greet",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "echo ああ"
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

## `<workspace>/#greet-and-done`

```json
{
  "task_display": {
    "package_name": "non-ascii-commands",
    "task_name": "greet-and-done",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "echo 你好 && FOO=é echo 🎉 done"
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

## `<workspace>/#greet-and-missing`

```json
{
  "task_display": {
    "package_name": "non-ascii-commands",
    "task_name": "greet-and-missing",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "echo こんにちは && vt run missing"
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

## `<workspace>/#greet-long`

```json
{
  "task_display": {
    "package_name": "non-ascii-commands",
    "task_name": "greet-long",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "echo あああ"
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

