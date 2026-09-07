# quoted_glob_stays_literal

Quoted pathname patterns remain literal for the child command to interpret on every platform.

## `vt run quoted-glob`

```
$ vtt print "packages/*/src" ⊘ cache disabled
packages/*/src
```
