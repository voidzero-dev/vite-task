# hook_run_directly_uses_depends_on_without_warning

A hook that runs as a task of its own uses its `dependsOn`, so there's nothing to warn about.

## `vt run prebuild`

```
$ vtt print setup ⊘ cache disabled
setup

$ vtt print prebuild ⊘ cache disabled
prebuild

---
vt run: 2/2 successful. (Run `vt run --last-details` for full details)
```
