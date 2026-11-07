# nested

## `vt run --dry-run --json ci`

`ci` runs two nested plans in turn. The first has its own edges, and the root's `build` in it runs a third plan, so `packages/a#build` is at two levels.

```
{
  "$schema": "<schema>",
  "version": 1,
  "workspaceRoot": "<workspace>",
  "plan": {
    "ci": {
      "package": ".",
      "name": "ci",
      "steps": [
        {
          "expandedFrom": "vt run -r build",
          "plan": {
            "build": {
              "package": ".",
              "name": "build",
              "steps": [
                {
                  "expandedFrom": "vt run --filter a build",
                  "plan": {
                    "packages/a#build": {
                      "package": "packages/a",
                      "name": "build",
                      "steps": [
                        {
                          "program": "<vtt>",
                          "args": [
                            "print",
                            "a"
                          ],
                          "cache": {
                            "status": "disabled"
                          }
                        }
                      ]
                    }
                  }
                }
              ]
            },
            "packages/a#build": {
              "package": "packages/a",
              "name": "build",
              "steps": [
                {
                  "program": "<vtt>",
                  "args": [
                    "print",
                    "a"
                  ],
                  "cache": {
                    "status": "disabled"
                  }
                }
              ]
            },
            "packages/b#build": {
              "package": "packages/b",
              "name": "build",
              "dependsOn": [
                "packages/a#build"
              ],
              "steps": [
                {
                  "program": "<vtt>",
                  "args": [
                    "print",
                    "b"
                  ],
                  "cache": {
                    "status": "disabled"
                  }
                }
              ]
            }
          }
        },
        {
          "expandedFrom": "vt run lint",
          "plan": {
            "lint": {
              "package": ".",
              "name": "lint",
              "steps": [
                {
                  "program": "<vtt>",
                  "args": [
                    "print",
                    "lint"
                  ],
                  "cache": {
                    "status": "disabled"
                  }
                }
              ]
            }
          }
        }
      ]
    }
  }
}
```

## `vt run --dry-run --json ci --flag`

Args after the task name go to the last command, a nested run here.

```
{
  "$schema": "<schema>",
  "version": 1,
  "workspaceRoot": "<workspace>",
  "plan": {
    "ci": {
      "package": ".",
      "name": "ci",
      "steps": [
        {
          "expandedFrom": "vt run -r build",
          "plan": {
            "build": {
              "package": ".",
              "name": "build",
              "steps": [
                {
                  "expandedFrom": "vt run --filter a build",
                  "plan": {
                    "packages/a#build": {
                      "package": "packages/a",
                      "name": "build",
                      "steps": [
                        {
                          "program": "<vtt>",
                          "args": [
                            "print",
                            "a"
                          ],
                          "cache": {
                            "status": "disabled"
                          }
                        }
                      ]
                    }
                  }
                }
              ]
            },
            "packages/a#build": {
              "package": "packages/a",
              "name": "build",
              "steps": [
                {
                  "program": "<vtt>",
                  "args": [
                    "print",
                    "a"
                  ],
                  "cache": {
                    "status": "disabled"
                  }
                }
              ]
            },
            "packages/b#build": {
              "package": "packages/b",
              "name": "build",
              "dependsOn": [
                "packages/a#build"
              ],
              "steps": [
                {
                  "program": "<vtt>",
                  "args": [
                    "print",
                    "b"
                  ],
                  "cache": {
                    "status": "disabled"
                  }
                }
              ]
            }
          }
        },
        {
          "expandedFrom": "vt run lint --flag",
          "plan": {
            "lint": {
              "package": ".",
              "name": "lint",
              "steps": [
                {
                  "program": "<vtt>",
                  "args": [
                    "print",
                    "lint",
                    "--flag"
                  ],
                  "cache": {
                    "status": "disabled"
                  }
                }
              ]
            }
          }
        }
      ]
    }
  }
}
```

## `vt run --dry-run --json preview`

A nested `vt run --dry-run` stays a spawned step, which a dry run doesn't start.

```
{
  "$schema": "<schema>",
  "version": 1,
  "workspaceRoot": "<workspace>",
  "plan": {
    "preview": {
      "package": ".",
      "name": "preview",
      "steps": [
        {
          "program": "<vt>",
          "args": [
            "run",
            "--dry-run",
            "lint"
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
