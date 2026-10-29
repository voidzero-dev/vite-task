# vt

This is the top-level library crate that orchestrates the full task lifecycle: loading the task graph, planning execution, running or replaying tasks , and reporting results.

It is consumed by two binaries:

- **[Vite+](https://github.com/voidzero-dev/vite-plus)** — the official product, where it powers `vp run`
- **[`vt_bin`](../vt_bin)** — internal `vt` CLI for developing and testing this repo without the full Vite+ stack

## Remote cache tracing

Set `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` to your collector's complete traces
URL, or `OTEL_EXPORTER_OTLP_ENDPOINT` to its base URL, to export the remote
cache requests made by `vp run`. With neither endpoint set, tracing is off.

The client uses the OpenTelemetry SDK and exports HTTP/JSON by default.
`OTEL_EXPORTER_OTLP_TRACES_PROTOCOL` (or `OTEL_EXPORTER_OTLP_PROTOCOL`) can
select `http/json` or `http/protobuf`. The corresponding standard
`OTEL_EXPORTER_OTLP[_TRACES]_HEADERS` and `OTEL_EXPORTER_OTLP[_TRACES]_TIMEOUT`
variables configure authentication headers and timeouts.

The default service name is `vite-task`. `OTEL_SERVICE_NAME` overrides it;
`OTEL_RESOURCE_ATTRIBUTES` adds attributes such as `service.namespace=build`
and can also supply a service name. Explicit `OTEL_SERVICE_NAME` takes
precedence. Use a stable service name to group runs, and resource attributes
to identify individual runners or developer machines.

Fetch, store and blob download requests emit client spans and send their W3C
`traceparent` to the cache server. A tracing server can then connect its server
span to the caller's client span in a service map. A normal cache miss is not
an error. Download spans cover the streamed body, ending when it is consumed,
fails, or is dropped. Exported spans contain the HTTP method, server host,
port, URL scheme, status code and error class; they contain no endpoint path,
query, request headers, cache keys, bodies or error source messages.

`OTEL_SDK_DISABLED=true` or `OTEL_TRACES_EXPORTER=none` turns export off even
when an endpoint is set. Exporter initialization failures and errors returned
by the SDK at shutdown leave task outcomes unchanged and emit a warning
without configuration values. Before returning its status, a session calls
the SDK's shutdown to drain pending batches, including when a task fails or
the command exits early.
