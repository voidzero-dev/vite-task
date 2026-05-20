# fail_if_no_match_succeeds_when_all_filters_match

With `--fail-if-no-match` and only matching filters, the run proceeds normally — strict mode does not change the success path.

## `vt run --filter @test/app --fail-if-no-match build`

```
~/packages/app$ vtt print built-app
built-app
```
