# fetch_envs_removed_ignore_uses_original_observation

Removing an explicit NEXT_* ignore leaves the merged built-in passthrough policy unchanged. The original observed value must survive an ignored cache hit so the next run detects its change. Explicit empty file inputs and outputs isolate the environment policy change.

## `NEXT_PUBLIC_API_URL=first vt run fetch-envs-removed-ignore`

populate: record the first value even though it is explicitly ignored

```
$ node scripts/fetch_envs.mjs --prefix NEXT_PUBLIC_
NEXT_PUBLIC_API_URL=first
```

## `NEXT_PUBLIC_API_URL=second vt run fetch-envs-removed-ignore`

cache hit: explicit ignore permits the changed value

```
$ node scripts/fetch_envs.mjs --prefix NEXT_PUBLIC_ ◉ cache hit, replaying
NEXT_PUBLIC_API_URL=first

---
vt run: cache hit.
```

## `vtt replace-file-content vite-task.json '"untrackedEnv": ["NEXT_*"]' '"untrackedEnv": []'`

remove the explicit ignore without changing the merged passthrough policy

```
```

## `NEXT_PUBLIC_API_URL=second vt run fetch-envs-removed-ignore`

cache miss: the original observation is now a dependency

```
$ node scripts/fetch_envs.mjs --prefix NEXT_PUBLIC_ ○ cache miss: env 'NEXT_PUBLIC_API_URL' changed, executing
NEXT_PUBLIC_API_URL=second
```
