# clean_and_rebuild_output_folder

The task reads `src/a.txt`, lists `out`, removes it, makes it again, writes `out/a.txt` and stats `out`, like a build tool cleaning its output folder before writing a new one.

## `vt run build`

```
$ vtt fs read src/a.txt list out rm out mkdir out write out/a.txt hi stat out
```

## `vtt replace-file-content src/a.txt v1 v2`

change an input, so the next run cleans and rebuilds the existing `out`

```
```

## `vt run build`

```
$ vtt fs read src/a.txt list out rm out mkdir out write out/a.txt hi stat out ○ cache miss: 'src/a.txt' modified, executing
```

## `vtt rm -r out`

remove the output folder, as on a clean checkout

```
```

## `vt run build`

```
$ vtt fs read src/a.txt list out rm out mkdir out write out/a.txt hi stat out ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vtt stat-file out/a.txt`

```
out/a.txt: exists
```
