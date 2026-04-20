# non_interactive_did_you_mean

In non-interactive mode, a typo that has close fuzzy matches should produce
"did you mean" suggestions.

## `vtt pipe-stdin -- vt run buid`

**Exit code:** 1

```
Task "buid" not found. Did you mean:
  app#build: echo build app
  lib#build: echo build lib
```
