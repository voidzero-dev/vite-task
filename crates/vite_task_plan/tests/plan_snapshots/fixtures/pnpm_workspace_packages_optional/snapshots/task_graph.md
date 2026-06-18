# task graph

```mermaid
flowchart TD
  task_0["<workspace>/#hello"]
```

## `<workspace>/#hello`

```json
{
  "task_display": {
    "package_name": "additional-envs",
    "task_name": "hello",
    "package_path": "<workspace>/"
  },
  "resolved_config": {
    "commands": [
      "echo hello"
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

