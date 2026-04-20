# interactive_select_task_from_lib

In the interactive selector launched from `packages/lib`, the first entry
should be a lib-owned task.

## `vt run`

**→ expect-milestone:** `task-select::0`

```
Select a task (↑/↓, Enter to run, type to search):

  › build           echo build lib
    lint            echo lint lib
    test            echo test lib
    typecheck       echo typecheck lib
    app (packages/app)
      build         echo build app
      lint          echo lint app
      test          echo test app
    task-select-test (workspace root)
      check         echo check root
      clean         echo clean root
      deploy        echo deploy root
  (…5 more)
```

**← write-key:** `enter`

```
Selected task: build
~/packages/lib$ echo build lib ⊘ cache disabled
build lib
```
