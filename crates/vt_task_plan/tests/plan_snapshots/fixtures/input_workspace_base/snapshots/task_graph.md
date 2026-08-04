# task graph

```mermaid
flowchart TD
  task_0["<workspace>/packages/app#build"]
```

## `<workspace>/packages/app#build`

```json
{
  "task_display": {
    "package_name": "app",
    "task_name": "build",
    "package_path": "<workspace>/packages/app"
  },
  "resolved_config": {
    "commands": [
      "echo build"
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
          "includes_auto": false,
          "positive_globs": [
            "configs/tsconfig.json",
            "packages/app/src/**"
          ],
          "negative_globs": [
            "dist/**"
          ]
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

