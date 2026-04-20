# interactive_search_with_hash_skips_reorder

Including `#` in the query means "target a specific package#task" and should
disable the current-package reordering in the filtered list.

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

**← write:** `lib#`

**→ expect-milestone:** `task-select:lib#:0`

```
Select a task (↑/↓, Enter to run, type to search): lib#

    lib (packages/lib)
  ›   build     echo build lib
      lint      echo lint lib
      test      echo test lib
      typecheck echo typecheck lib
```

**← write-key:** `enter`

```
Selected task: lib#build
~/packages/lib$ echo build lib ⊘ cache disabled
build lib
```
