# interactive_ctrl_u_clears_query

Ctrl+U should clear the current query and reset the selection, while other control characters are ignored and Shift characters remain searchable.

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

**← write-key:** `ctrl-w`

**← write-key:** `backspace`

**→ expect-milestone:** `task-select:li:0`

```
Select a task (↑/↓, Enter to run, type to search): li

  › lint        echo lint app
    lib (packages/lib)
      build     echo build lib
      lint      echo lint lib
      test      echo test lib
      typecheck echo typecheck lib
    task-select-test (workspace root)
      validate  echo validate root
```

**← write:** `N`

**→ expect-milestone:** `task-select:liN:0`

```
Select a task (↑/↓, Enter to run, type to search): liN

  › lint   echo lint app
    lib (packages/lib)
      lint echo lint lib
```

**← write-key:** `down`

**→ expect-milestone:** `task-select:liN:1`

```
Select a task (↑/↓, Enter to run, type to search): liN

    lint   echo lint app
    lib (packages/lib)
  ›   lint echo lint lib
```

**← write-key:** `ctrl-u`

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
