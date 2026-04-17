# whitespace_split_filter_warns_for_unmatched_token

Tests for unmatched --filter warnings on stderr

## `vt run --filter '@test/app nope' build`

```
No packages matched the filter: nope
~/packages/app$ vtt print built-app
built-app
```
