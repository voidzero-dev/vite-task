# empty_input___miss_on_command_change

Even with `input: []`, changing the task's command should still invalidate
the cache.

## `vt run empty-inputs`

```
$ vtt print-file ./src/main.ts
export const main = 'initial';
```

## `vtt replace-file-content vite-task.json 'vtt print-file ./src/main.ts' 'vtt print-file src/utils.ts'`

```
```

## `vt run empty-inputs`

```
$ vtt print-file src/utils.ts ○ cache miss: args changed, executing
export const utils = 'initial';
```
