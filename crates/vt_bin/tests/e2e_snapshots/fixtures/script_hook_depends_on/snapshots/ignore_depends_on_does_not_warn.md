# ignore_depends_on_does_not_warn

`--ignore-depends-on` already asks to skip dependencies, so hooks don't warn.

## `vt run --ignore-depends-on build`

```
$ vtt print prebuild ⊘ cache disabled
prebuild

$ vtt print build ⊘ cache disabled
build

$ vtt print postbuild ⊘ cache disabled
postbuild

---
vt run: 3/3 successful. (Run `vt run --last-details` for full details)
```
