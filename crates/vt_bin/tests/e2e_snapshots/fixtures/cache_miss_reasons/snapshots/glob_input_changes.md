# glob_input_changes

Modifying, adding, or removing files matched by a glob `input` pattern should each invalidate the cache.

## `vt run glob-test`

cache miss

```
$ vtt print glob-test
glob-test
```

## `vtt replace-file-content test.txt initial modified`

modify glob input

```
```

## `vt run glob-test`

cache miss: input modified

```
$ vtt print glob-test ○ cache miss: 'test.txt' modified, executing
glob-test
```

## `vtt write-file new.txt 'new content'`

add a new .txt file

```
```

## `vt run glob-test`

cache miss: input added

```
$ vtt print glob-test ○ cache miss: 'new.txt' added in workspace root, executing
glob-test
```

## `vtt rm extra.txt`

remove a .txt file

```
```

## `vt run glob-test`

cache miss: input removed

```
$ vtt print glob-test ○ cache miss: 'extra.txt' removed from workspace root, executing
glob-test
```
