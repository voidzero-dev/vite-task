# multi_task_silent_grouped_suppresses_block_headers

`vt run -r --silent --log grouped` should suppress the `── [pkg#task] ──` block headers that grouped mode normally prints, leaving only the tasks' own output.

## `vt run -r --silent --log grouped build`

```
built-a
built-b
```
