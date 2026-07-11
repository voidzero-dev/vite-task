# constrained_dev_shm

Mounting a one-page `/dev/shm` reproduces the SIGBUS seen before fspy moved its shared-memory backing to memfd.

## `vtt small_dev_shm vt run stress`

**Exit code:** 135

```
$ vtt stat_long_filename 1048576
```
