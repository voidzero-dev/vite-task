# non_interactive_list_tasks

With piped stdin (non-interactive), `vt run` with no task argument should
list all available tasks instead of launching the picker.

## `vtt pipe-stdin -- vt run`

```
  check: echo check root
  clean: echo clean root
  deploy: echo deploy root
  docs: echo docs root
  format: echo format root
  hello: echo hello from root
  run-typo-task: vt run nonexistent-xyz
  validate: echo validate root
  app#build: echo build app
  app#lint: echo lint app
  app#test: echo test app
  lib#build: echo build lib
  lib#lint: echo lint lib
  lib#test: echo test lib
  lib#typecheck: echo typecheck lib
```
