# write_outside_workspace_through_dotdot

The task makes `../site` and writes `../site/index.html`, outside the workspace, like `cp -r dist ../site` publishing a build next to it. `..` is the folder holding the staged workspace.

## `vt run deploy`

**Exit code:** 1

```
$ vtt fs mkdir ../site write ../site/index.html hi
✗ Cache update failed: paths in archives must not have `..` when setting path for
```

## `vt run deploy`

**Exit code:** 1

```
$ vtt fs mkdir ../site write ../site/index.html hi
✗ Cache update failed: paths in archives must not have `..` when setting path for
```
