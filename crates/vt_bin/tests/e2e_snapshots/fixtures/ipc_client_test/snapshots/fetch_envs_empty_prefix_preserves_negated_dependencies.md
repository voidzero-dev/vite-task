# fetch_envs_empty_prefix_preserves_negated_dependencies

An empty prefix queries every ambient variable. Negating VITE_* from untrackedEnv keeps its values, additions and removals as cache dependencies. Only VITE_* results are printed so platform-specific ambient values stay out of the snapshot.

## `VITE_FEATURE=first vt run fetch-envs-all-untracked-except-vite`

populate: VITE_FEATURE is excluded from the untracked wildcard

```
$ node scripts/fetch_envs.mjs --prefix "" --print-prefix VITE_
VITE_FEATURE=first
```

## `VITE_FEATURE=second vt run fetch-envs-all-untracked-except-vite`

cache miss: a negated dependency changed

```
$ node scripts/fetch_envs.mjs --prefix "" --print-prefix VITE_ ○ cache miss: env 'VITE_FEATURE' changed, executing
VITE_FEATURE=second
```

## `VITE_FEATURE=second VITE_ADDED=added vt run fetch-envs-all-untracked-except-vite`

cache miss: a negated dependency was added

```
$ node scripts/fetch_envs.mjs --prefix "" --print-prefix VITE_ ○ cache miss: env 'VITE_ADDED' added, executing
VITE_ADDED=added
VITE_FEATURE=second
```

## `VITE_FEATURE=second vt run fetch-envs-all-untracked-except-vite`

cache miss: a negated dependency was removed

```
$ node scripts/fetch_envs.mjs --prefix "" --print-prefix VITE_ ○ cache miss: env 'VITE_ADDED' removed, executing
VITE_FEATURE=second
```
