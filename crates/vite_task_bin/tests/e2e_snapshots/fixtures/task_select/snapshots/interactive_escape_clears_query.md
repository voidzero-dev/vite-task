# interactive_escape_clears_query

Escape in the selector should clear the current query and restore the
unfiltered list at cursor position 0.

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

**← write:** `lin`

**→ expect-milestone:** `task-select:lin:0`

```
Select a task (↑/↓, Enter to run, type to search): lin

  › lint   echo lint app
    lib (packages/lib)
      lint echo lint lib
```

**← write-key:** `escape`

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

**← write-key:** `enter`

```
Selected task: build
~/packages/app$ echo build app ⊘ cache disabled
build app
```
