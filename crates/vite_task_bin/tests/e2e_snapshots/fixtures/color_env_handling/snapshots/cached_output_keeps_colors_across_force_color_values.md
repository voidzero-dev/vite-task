# cached_output_keeps_colors_across_force_color_values

A task that emits ANSI escape sequences is run twice. Each cache entry
carries the raw coloured bytes — the runner spawns cached tasks with
`FORCE_COLOR=1` regardless of the parent's value — and the reporter
strips colours at the writer level only when the user's terminal cannot
render them.

Plain-text snapshot steps use the default `vt100::Screen::contents()`
renderer, which flattens any rendered ANSI styling into plain characters
(so colors don't pollute snapshots). The two `formatted-snapshot = true`
steps switch to `contents_formatted()`, then route every non-newline byte
through `std::ascii::escape_default` so escape sequences appear as
`\xNN`. Newlines are kept literal so the snapshot stays multi-line.
Those two steps prove that the cached bytes contained colour all along
— even when the corresponding initial run had displayed them in plain
text.

## `FORCE_COLOR=1 vt run print-colored-a`

task A — cache miss; parent FORCE_COLOR=1; formatted snapshot proves the child emitted ANSI codes

```
\x1b[?25h\x1b[m\x1b[H\x1b[J\x1b[34m$ vtt print-color red hello-world\r
\x1b[31mhello-world\x1b[5;1H\x1b[m
```

## `FORCE_COLOR=0 vt run print-colored-a`

task A — cache hit replayed; plain snapshot collapses any colour styling to text

```
$ vtt print-color red hello-world ◉ cache hit, replaying
hello-world

---
vt run: cache hit.
```

## `FORCE_COLOR=0 vt run print-colored-b`

task B — cache miss; parent FORCE_COLOR=0, plain snapshot collapses styling. The cache still records the coloured bytes.

```
$ vtt print-color blue hello-again
hello-again
```

## `FORCE_COLOR=1 vt run print-colored-b`

task B — cache hit replayed; formatted snapshot proves the cached bytes were coloured, even though the cache-miss run (above) showed them as plain text

```
\x1b[?25h\x1b[m\x1b[H\x1b[J\x1b[34m$ vtt print-color blue hello-again\x1b[m \x1b[32m\xe2\x97\x89\x1b[m \x1b[90mcache hit, replaying\r
\x1b[34mhello-again\x1b[4;1H\x1b[90m---\r
\x1b[34;1mvt run:\x1b[m cache hit, \x1b[32;1m<duration>\x1b[m saved.\x1b[7;1H
```
