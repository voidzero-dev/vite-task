# indented_in_terminal

## `vt run --json`

```
{
  "$schema": "<schema>",
  "version": 1,
  "workspaceRoot": "<workspace>",
  "packages": {
    ".": {
      "name": "app"
    },
    "packages/lib": {
      "name": "lib"
    }
  },
  "tasks": {
    "build": {
      "package": ".",
      "name": "build",
      "command": "vtt print built",
      "dependsOn": [
        "packages/lib#build"
      ]
    },
    "packages/lib#build": {
      "package": "packages/lib",
      "name": "build",
      "command": "vtt print built lib"
    }
  }
}
```
