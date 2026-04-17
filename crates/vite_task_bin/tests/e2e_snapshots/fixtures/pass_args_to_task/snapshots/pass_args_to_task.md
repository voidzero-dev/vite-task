# pass_args_to_task

Tests that arguments after task name should be passed to the task
https://github.com/voidzero-dev/vite-task/issues/285

## `vt run echo --help`

```
$ echo --help ⊘ cache disabled
--help
```

## `vt run echo --version`

```
$ echo --version ⊘ cache disabled
--version
```

## `vt run echo -v`

```
$ echo -v ⊘ cache disabled
-v
```

## `vt run echo -a`

```
$ echo -a ⊘ cache disabled
-a
```
