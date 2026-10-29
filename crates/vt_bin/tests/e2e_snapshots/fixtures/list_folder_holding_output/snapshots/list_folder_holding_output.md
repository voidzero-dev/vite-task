# list_folder_holding_output

The task makes `gen`, writes `gen/a.txt` and lists the package root, like a code generator listing the folder its output goes into.

## `vt run build`

```
$ vtt fs mkdir gen write gen/a.txt hi list .
```

## `vtt rm -r gen`

remove the output folder, as on a clean checkout

```
```

## `vt run build`

```
$ vtt fs mkdir gen write gen/a.txt hi list . ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vtt write-file notes.txt x`

other entries of the listing still count

```
```

## `vt run build`

```
$ vtt fs mkdir gen write gen/a.txt hi list . ○ cache miss: 'notes.txt' added in workspace root, executing
```
