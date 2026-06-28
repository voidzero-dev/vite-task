# init_cwd_points_to_invocation_cwd

Tasks run from their package directory, but INIT_CWD should point to the directory where vt was invoked.

## `vt run print-init-cwd`

```
~/packages/app$ vtt print-env INIT_CWD
<workspace>/packages/app/src
```
