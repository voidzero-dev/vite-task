# steps

## `vt run --dry-run --json build --watch`

Args after the task name are part of the step's args.

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
            "build",
            "--watch"
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

## `vtt pipe-stdout -- vt run --dry-run --json pipe`

A command with shell syntax runs through the shell: `/bin/sh -c`, or `cmd.exe /d /s /c` on Windows.

```
--- stdout ---
{"$schema":"<schema>","version":1,"workspaceRoot":"<workspace>","plan":{"packages/web#pipe":{"package":"packages/web","name":"pipe","steps":[{"program":"<shell>","args":["<shell flags>","vtt print a | vtt print b"],"cache":{"status":"noBaseline"}}]}}}
```

## `vt run --dry-run --json cd`

A `cd` changes the cwd of the steps after it.

```
{
  "$schema": "<schema>",
  "version": 1,
  "workspaceRoot": "<workspace>",
  "plan": {
    "packages/web#cd": {
      "package": "packages/web",
      "name": "cd",
      "steps": [
        {
          "program": "<vtt>",
          "args": [
            "print",
            "in-src"
          ],
          "cwd": "packages/web/src",
          "cache": {
            "status": "noBaseline"
          }
        }
      ]
    }
  }
}
```

## `vt run --dry-run --json in-src`

So does the task's `cwd` setting.

```
{
  "$schema": "<schema>",
  "version": 1,
  "workspaceRoot": "<workspace>",
  "plan": {
    "packages/web#in-src": {
      "package": "packages/web",
      "name": "in-src",
      "steps": [
        {
          "program": "<vtt>",
          "args": [
            "print",
            "in-src"
          ],
          "cwd": "packages/web/src",
          "cache": {
            "status": "noBaseline"
          }
        }
      ]
    }
  }
}
```

## `vt run --dry-run --json steps`

Each `&&` part is a step, and a built-in such as `echo` has no `cache`.

```
{
  "$schema": "<schema>",
  "version": 1,
  "workspaceRoot": "<workspace>",
  "plan": {
    "packages/web#steps": {
      "package": "packages/web",
      "name": "steps",
      "steps": [
        {
          "program": "<vtt>",
          "args": [
            "print",
            "one"
          ],
          "cache": {
            "status": "noBaseline"
          }
        },
        {
          "program": "echo",
          "args": [
            "two"
          ]
        }
      ]
    }
  }
}
```
