# nested_vt_quoted_glob_stays_command_owned

A quoted pathname pattern remains one literal argument when passed to a nested Vite+ command, so the command can implement portable glob semantics itself.

## `vt run nested-vt-quoted-glob`

```
$ vtt print prefix 'packages/*/src' ⊘ cache disabled
prefix packages/*/src
```
