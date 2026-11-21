# tasks

## `vt run --dry-run --json -r build`

Package edges from `-r`, `pre` and `post` scripts as hooks, a program inside the workspace and one outside it, and a package path with `#` in its ID.

```
{
  "$schema": "<schema>",
  "version": 1,
  "workspaceRoot": "<workspace>",
  "plan": {
    "packages/p%23q#build": {
      "package": "packages/p#q",
      "name": "build",
      "steps": [
        {
          "program": "echo",
          "args": [
            "built"
          ]
        }
      ]
    },
    "packages/ui#build": {
      "package": "packages/ui",
      "name": "build",
      "steps": [
        {
          "program": "<vtt>",
          "args": [
            "print",
            "pre"
          ],
          "hook": "prebuild",
          "cache": {
            "status": "disabled"
          }
        },
        {
          "program": "<vtt>",
          "args": [
            "print",
            "build"
          ],
          "cache": {
            "status": "disabled"
          }
        },
        {
          "program": "<vtt>",
          "args": [
            "print",
            "post"
          ],
          "hook": "postbuild",
          "cache": {
            "status": "disabled"
          }
        }
      ]
    },
    "packages/web#build": {
      "package": "packages/web",
      "name": "build",
      "dependsOn": [
        "packages/ui#build"
      ],
      "steps": [
        {
          "program": "packages/web/node_modules/.bin/tool",
          "args": [
            "build"
          ],
          "cache": {
            "status": "noBaseline"
          }
        }
      ]
    }
  }
}
```

## `vt run --dry-run --json --parallel -r build`

`--parallel` drops the edges.

```
{
  "$schema": "<schema>",
  "version": 1,
  "workspaceRoot": "<workspace>",
  "plan": {
    "packages/p%23q#build": {
      "package": "packages/p#q",
      "name": "build",
      "steps": [
        {
          "program": "echo",
          "args": [
            "built"
          ]
        }
      ]
    },
    "packages/ui#build": {
      "package": "packages/ui",
      "name": "build",
      "steps": [
        {
          "program": "<vtt>",
          "args": [
            "print",
            "pre"
          ],
          "hook": "prebuild",
          "cache": {
            "status": "disabled"
          }
        },
        {
          "program": "<vtt>",
          "args": [
            "print",
            "build"
          ],
          "cache": {
            "status": "disabled"
          }
        },
        {
          "program": "<vtt>",
          "args": [
            "print",
            "post"
          ],
          "hook": "postbuild",
          "cache": {
            "status": "disabled"
          }
        }
      ]
    },
    "packages/web#build": {
      "package": "packages/web",
      "name": "build",
      "steps": [
        {
          "program": "packages/web/node_modules/.bin/tool",
          "args": [
            "build"
          ],
          "cache": {
            "status": "noBaseline"
          }
        }
      ]
    }
  }
}
```

## `vt run --dry-run --json --no-cache --filter @acme/web build`

`--no-cache` disables caching for every step.

```
{
  "$schema": "<schema>",
  "version": 1,
  "workspaceRoot": "<workspace>",
  "plan": {
    "packages/web#build": {
      "package": "packages/web",
      "name": "build",
      "steps": [
        {
          "program": "packages/web/node_modules/.bin/tool",
          "args": [
            "build"
          ],
          "cache": {
            "status": "disabled"
          }
        }
      ]
    }
  }
}
```

## `vtt pipe-stdout -- vt run --dry-run --json --cache --filter @acme/ui build`

`--cache` turns caching on for scripts, which are uncached by default.

```
--- stdout ---
{"$schema":"<schema>","version":1,"workspaceRoot":"<workspace>","plan":{"packages/ui#build":{"package":"packages/ui","name":"build","steps":[{"program":"<vtt>","args":["print","pre"],"hook":"prebuild","cache":{"status":"noBaseline"}},{"program":"<vtt>","args":["print","build"],"cache":{"status":"noBaseline"}},{"program":"<vtt>","args":["print","post"],"hook":"postbuild","cache":{"status":"noBaseline"}}]}}}
```

## `vt run --dry-run --json b%c`

`%` in a task name is encoded in its ID.

```
{
  "$schema": "<schema>",
  "version": 1,
  "workspaceRoot": "<workspace>",
  "plan": {
    "b%25c": {
      "package": ".",
      "name": "b%c",
      "steps": [
        {
          "program": "echo",
          "args": [
            "id"
          ]
        }
      ]
    }
  }
}
```
