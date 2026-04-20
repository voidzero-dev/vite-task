# whitespace_split_filter_warns_for_unmatched_token

A whitespace-split `--filter` argument should warn about each of its individual
unmatched tokens.

## `vt run --filter '@test/app nope' build`

```
No packages matched the filter: nope
~/packages/app$ vtt print built-app
built-app
```
