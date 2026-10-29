# stat_own_output_folder

The task makes `out`, writes `out/a.txt` and stats `out`, like a build tool checking its output folder.

## `vt run build`

```
$ vtt fs mkdir out write out/a.txt hi stat out
```

## `vtt rm -r out`

remove the output folder, as on a clean checkout

```
```

## `vt run build`

```
$ vtt fs mkdir out write out/a.txt hi stat out ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vtt stat-file out/a.txt`

```
out/a.txt: exists
```
