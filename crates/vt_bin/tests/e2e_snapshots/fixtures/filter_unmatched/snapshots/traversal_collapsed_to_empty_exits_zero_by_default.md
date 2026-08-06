# traversal_collapsed_to_empty_exits_zero_by_default

`{.}^...` selects the dependencies of the current package, excluding itself. On a leaf with no workspace deps the expression collapses to zero matches — a legitimate no-op rather than a typo — so the run warns and exits 0.

## `vt run --filter {.}^... build`

```
No packages matched the filter: {.}^...
```
