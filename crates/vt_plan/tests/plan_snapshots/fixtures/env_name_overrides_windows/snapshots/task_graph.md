# task graph

```mermaid
flowchart TD
  task_0["<workspace>/#inner"]
  task_1["<workspace>/#outer"]
```

## `<workspace>/#inner`

```json
{
  "task_display": {
    "package_name": "env-name-overrides",
    "task_name": "inner",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "Foo=first FOO=last vp_run=custom force_color=0 vt tool print-env FOO"
    ],
    "resolved_options": {
      "cwd": "<workspace>/",
      "cache_config": {
        "remote_cache": true,
        "env_config": {
          "fingerprinted_envs": [
            "foo",
            "FOO",
            "Foo"
          ],
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

## `<workspace>/#outer`

```json
{
  "task_display": {
    "package_name": "env-name-overrides",
    "task_name": "outer",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "FOO=outer vt run inner"
    ],
    "resolved_options": {
      "cwd": "<workspace>/",
      "cache_config": {
        "remote_cache": true,
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

