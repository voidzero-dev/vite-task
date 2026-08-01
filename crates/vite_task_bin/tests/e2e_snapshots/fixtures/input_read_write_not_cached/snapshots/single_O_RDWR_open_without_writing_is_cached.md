# single_O_RDWR_open_without_writing_is_cached

Opening a file `O_RDWR` and never writing to it leaves the file untouched, so it is a read and not a read-write overlap. Write capability alone must not block caching: formatters such as Biome open a clean source file read-write on every run and only write when there is something to fix, and lock files across cargo, rustc and Parcel are opened `O_RDWR` and only ever flocked. A mutation requires truncation, exclusive creation, or being a rename destination.

## `vt run task`

```
~/packages/touch-pkg$ vtt touch-file src/data.txt
```

## `vt run task`

```
~/packages/touch-pkg$ vtt touch-file src/data.txt ◉ cache hit, replaying

---
vt run: cache hit.
```
