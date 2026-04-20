# interactive_select_task

In interactive mode, navigating down one entry and pressing Enter should
select the second task.

## `vt run`

**→ expect-milestone:** `task-select::0`

```
Select a task (↑/↓, Enter to run, type to search):

  › build           echo build app
    lint            echo lint app
    test            echo test app
    lib (packages/lib)
      build         echo build lib
      lint          echo lint lib
      test          echo test lib
      typecheck     echo typecheck lib
    task-select-test (workspace root)
      check         echo check root
      clean         echo clean root
      deploy        echo deploy root
  (…5 more)
```

**← write-key:** `down`

**→ expect-milestone:** `task-select::1`

```
Select a task (↑/↓, Enter to run, type to search):

    build           echo build app
  › lint            echo lint app
    test            echo test app
    lib (packages/lib)
      build         echo build lib
      lint          echo lint lib
      test          echo test lib
      typecheck     echo typecheck lib
    task-select-test (workspace root)
      check         echo check root
      clean         echo clean root
      deploy        echo deploy root
  (…5 more)
```

**← write-key:** `enter`

```
Selected task: lint
~/packages/app$ echo lint app ⊘ cache disabled
lint app
```
