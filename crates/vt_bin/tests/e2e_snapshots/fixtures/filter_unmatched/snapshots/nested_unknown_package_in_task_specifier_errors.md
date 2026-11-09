# nested_unknown_package_in_task_specifier_errors

A nested `vt run nonexistent#build` fails the outer task's planning too, unlike the nested `--filter nonexistent` above. The error chain names the nested command and the missing package.

## `vt run run-unknown-package`

**Exit code:** 1

```
error: Failed to plan tasks from `vt run nonexistent#build` in task filter-unmatched-test#run-unknown-package
* Package 'nonexistent' not found in the workspace
```
