# powershell_package_shim_keeps_pathext

Cached Windows tasks run with a sanitized environment, and pnpm-style package
`.cmd` shims in `node_modules/.bin` are rewritten at plan time to invoke their
`.ps1` sibling via `powershell.exe -File`. PowerShell needs `PATHEXT` in that
environment to launch native executables — even when the executable name is
written out with a `.exe` extension — so `PATHEXT` must be preserved in the
default untracked env passthrough.

## `vt run probe`

rewritten to `powershell.exe -File probe-cli.ps1`; the shim launches `vtt.exe write-file` to write `marker.txt`

```
$ probe-cli
shim-ran
```

## `vtt print-file marker.txt`

marker is present only if the shim's `vtt.exe` launch succeeded

```
shim-ran
```
