# hooks_warn_that_depends_on_is_ignored

`dependsOn` of a pre or post hook doesn't apply when the hook runs inside its script, so `setup` doesn't run and each hook warns.

## `vt run build`

```
`dependsOn` of @test/script-hook-depends-on#prebuild is ignored when it runs as a hook of @test/script-hook-depends-on#build. Add it to `build` in `run.scripts`.
`dependsOn` of @test/script-hook-depends-on#postbuild is ignored when it runs as a hook of @test/script-hook-depends-on#build. Add it to `build` in `run.scripts`.
$ vtt print prebuild ⊘ cache disabled
prebuild

$ vtt print build ⊘ cache disabled
build

$ vtt print postbuild ⊘ cache disabled
postbuild

---
vt run: 3/3 successful. (Run `vt run --last-details` for full details)
```
