# vitest_browser_caches_inputs_and_restores_outputs

Vitest browser mode runs in headless Chromium with explicit automatic input and output tracking. A source change must invalidate the cache, while an unchanged run must restore the browser command's generated output without rerunning Vitest. Playwright's bundled Chromium is unavailable on musl, so this browser-specific case is skipped only there.

## `vt run --cache test`

first browser run: cache miss writes dist/result.json

```
$ vitest run

 RUN  v4.1.10 <workspace>

 ✓  chromium  src/greeting.test.js (1 test) <duration>
JSON report written to <workspace>/dist/result.json
```

## `vtt grep-file dist/result.json 'hello browser alpha'`

Vitest's JSON report contains the initial imported value

```
dist/result.json: found "hello browser alpha"
```

## `vtt rm dist/result.json`

remove the generated output so restoration is observable

```
```

## `vt run --cache test`

unchanged inputs: cache hit restores dist/result.json

```
$ vitest run ◉ cache hit, replaying

 RUN  v4.1.10 <workspace>

 ✓  chromium  src/greeting.test.js (1 test) <duration>
JSON report written to <workspace>/dist/result.json

---
vt run: cache hit.
```

## `vtt grep-file dist/result.json 'hello browser alpha'`

the automatic output archive restored Vitest's report

```
dist/result.json: found "hello browser alpha"
```

## `vtt replace-file-content src/greeting.js 'hello browser alpha' 'hello browser bravo'`

modify a module loaded by the browser

```
```

## `vt run --cache test`

automatic input changed: cache miss reruns the browser test

```
$ vitest run ○ cache miss: 'src/greeting.js' modified, executing

 RUN  v4.1.10 <workspace>

 ✓  chromium  src/greeting.test.js (1 test) <duration>
JSON report written to <workspace>/dist/result.json
```

## `vtt grep-file dist/result.json 'hello browser bravo'`

the rerun's JSON report contains the modified imported value

```
dist/result.json: found "hello browser bravo"
```
