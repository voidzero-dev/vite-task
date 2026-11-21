# checking_local_entry_doesnt_block_other_tasks

## `vt run --ignore-depends-on read`

read reads input.txt, and its entry is recorded locally.

```
$ vtt print-file input.txt
input
```

## `vt run test`

Other tasks run while read's local entry is checked, and then it hits.

```
$ vtt hold-reads input.txt .barrier ⊘ cache disabled
$ vtt barrier .barrier held 2 ⊘ cache disabled
$ vtt print other ⊘ cache disabled
other
$ vtt barrier .barrier released 2 ⊘ cache disabled
$ vtt print-file input.txt ◉ cache hit, replaying
input
$ vtt print done ⊘ cache disabled
done
---
vt run: 1/1 cache hit (100%), 6/6 successful. (Run `vt run --last-details` for full details)
```
