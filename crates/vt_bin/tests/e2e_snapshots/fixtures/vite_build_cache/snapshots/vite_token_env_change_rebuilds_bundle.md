# vite_token_env_change_rebuilds_bundle

Vite's default VITE_ prefix makes VITE_MAPBOX_TOKEN a bundle input even though *_TOKEN is a default untracked environment pattern. Changing only the ambient token must invalidate the cache and replace the old token in the emitted JavaScript without manual cache configuration.

## `VITE_MAPBOX_TOKEN=mapbox-token-alpha vt run --cache build`

first run: the ambient token is substituted into the bundle

```
$ vite build
```

## `vtt grep-file dist/assets/main.js mapbox-token-alpha`

the first bundle contains the first token

```
dist/assets/main.js: found "mapbox-token-alpha"
```

## `VITE_MAPBOX_TOKEN=mapbox-token-bravo vt run --cache build`

cache miss: the token changed while every source file stayed the same

```
$ vite build ○ cache miss: env 'VITE_MAPBOX_TOKEN' changed, executing
```

## `vtt grep-file dist/assets/main.js mapbox-token-alpha`

the rebuilt bundle no longer contains the first token

```
dist/assets/main.js: missing "mapbox-token-alpha"
```

## `vtt grep-file dist/assets/main.js mapbox-token-bravo`

the rebuilt bundle contains the new token

```
dist/assets/main.js: found "mapbox-token-bravo"
```
