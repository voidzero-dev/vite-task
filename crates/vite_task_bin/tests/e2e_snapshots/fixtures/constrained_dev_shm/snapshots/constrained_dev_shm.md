# constrained_dev_shm

With fspy's shared-memory backing moved to memfd, file-access tracking succeeds without consuming a one-page `/dev/shm` mount.

## `vtt small_dev_shm vt run stress`

```
$ vtt stat_long_filename 1048576
```
