# github_oidc_upload

## `VP_REMOTE_CACHE=read-write vtt oidc-remote-cache vt run verify`

The first upload requests an OIDC token for the endpoint, and the second reuses it. Fetches send no token.

```
$ vtt write-file dist/output.txt built

$ vtt print verified
verified

---
vt run: 0/2 cache hit (0%). (Run `vt run --last-details` for full details)
[remote-cache] POST /fetch 404
[github-oidc] GET /token 200
[remote-cache] POST /store 200
[remote-cache] POST /fetch 404
[remote-cache] POST /store 200
```
