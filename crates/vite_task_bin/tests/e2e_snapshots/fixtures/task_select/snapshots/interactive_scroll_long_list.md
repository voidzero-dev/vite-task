# interactive_scroll_long_list

With a list taller than the 8-row page, navigating past the viewport should
scroll down, and navigating back should scroll the view up to index 0.

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

**← write-key:** `down`

**← write-key:** `down`

**← write-key:** `down`

**← write-key:** `down`

**← write-key:** `down`

**← write-key:** `down`

**← write-key:** `down`

**→ expect-milestone:** `task-select::8`

```
Select a task (↑/↓, Enter to run, type to search):

    build           echo build app
    lint            echo lint app
    test            echo test app
    lib (packages/lib)
      build         echo build lib
      lint          echo lint lib
      test          echo test lib
      typecheck     echo typecheck lib
    task-select-test (workspace root)
      check         echo check root
  ›   clean         echo clean root
      deploy        echo deploy root
  (…5 more)
```

**← write-key:** `up`

**← write-key:** `up`

**← write-key:** `up`

**← write-key:** `up`

**← write-key:** `up`

**← write-key:** `up`

**← write-key:** `up`

**← write-key:** `up`

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
