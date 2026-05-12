# cached_output_keeps_colors_across_force_color_values

A task that emits ANSI escape sequences is run twice. Each cache entry
carries the raw coloured bytes — the runner spawns cached tasks with
`FORCE_COLOR=1` regardless of the parent's value — and the reporter
strips colours at the writer level only when the user's terminal cannot
render them.

Plain-text snapshot steps use the default `vt100::Screen::contents()`
renderer, which flattens any rendered ANSI styling into plain characters
(so colors don't pollute snapshots). The two `formatted-snapshot = true`
steps switch to `contents_formatted()`, which re-emits the screen with
inline SGR escapes (rendered as `\\e[…m` for review readability). Those
two steps prove that the cached bytes contained colour all along — even
when the corresponding initial run had displayed them in plain text.

## `FORCE_COLOR=1 vt run print-colored-a`

task A — cache miss; parent FORCE_COLOR=1; formatted snapshot proves the child emitted ANSI codes

```
\e[?25h\e[m\e[H\e[J\e[34m$ vtt print-color red hello-world\r
\e[31mhello-world\e[5;1H\e[m
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
\e[?25h\e[m\e[H\e[J\e[34m$ vtt print-color blue hello-again\e[m \e[32m◉\e[m \e[90mcache hit, replaying\r
\e[34mhello-again\e[4;1H\e[90m---\r
\e[34;1mvt run:\e[m cache hit, \e[32;1m<duration>\e[m saved.\e[7;1H
```
