# stdin_and_tty

Reporting works with inherited terminal descriptors and does not change stdin behavior.

## `vt run --no-cache tty`

```
$ vtt report-unchanged tty ⊘ cache disabled
command ran
stdin:tty
stdout:tty
stderr:tty
◉ unchanged (reported by task)

---
vt run: unchanged (reported by task).
```

## `vt run tty`

```
$ vtt report-unchanged tty
command ran
stdin:not-tty
stdout:not-tty
stderr:not-tty
◉ unchanged (reported by task)

---
vt run: unchanged (reported by task).
```

## `vtt pipe-stdin from-stdin -- vt run --no-cache stdin`

```
$ vtt report-unchanged stdin ⊘ cache disabled
command ran
from-stdin
◉ unchanged (reported by task)

---
vt run: unchanged (reported by task).
```

## `vtt pipe-stdin from-stdin -- vt run stdin`

```
$ vtt report-unchanged stdin
command ran
◉ unchanged (reported by task)

---
vt run: unchanged (reported by task).
```
