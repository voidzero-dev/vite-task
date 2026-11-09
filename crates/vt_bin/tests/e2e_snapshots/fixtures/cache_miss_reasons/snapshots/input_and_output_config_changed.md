# input_and_output_config_changed

When a task's `input` and `output` settings change together, the miss lists both.

## `vt run test`

```
$ vtt print-file test.txt
initial content
```

## `vtt replace-file-content vite-task.json '"env": ["MY_ENV"]' '"env": ["MY_ENV"], "input": ["test.txt"], "output": []'`

```
```

## `vt run -v test`

The inline reason is the first change. The summary lists both.

```
$ vtt print-file test.txt ○ cache miss: input configuration changed, executing
initial content


━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 task • 0 cache hits • 1 cache miss
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] cache-miss-reasons#test: $ vtt print-file test.txt ✓
      → Cache miss: input configuration changed
                    output configuration changed
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```
