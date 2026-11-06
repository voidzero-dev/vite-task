# write_outside_workspace_through_dotdot

The task makes `../site` and writes `../site/index.html`, outside the workspace, like `cp -r dist ../site` publishing a build next to it. `..` is the folder holding the staged workspace.

## `vt run deploy`

```
$ vtt fs mkdir ../site write ../site/index.html hi
```

## `vt run deploy`

```
$ vtt fs mkdir ../site write ../site/index.html hi ◉ cache hit, replaying

---
vt run: cache hit.
```
