# vt_remote_cache

Client for the [remote cache server API](https://github.com/voidzero-dev/vite-task/pull/713). It treats keys, values, and blobs as opaque bytes. Encoding task cache entries into them is up to the caller.

`Client::new` takes the configured endpoint, which can include a namespace path, such as `https://cache.example.com/projects/my-project`. Each operation appends its route to that path, so a store goes to `https://cache.example.com/projects/my-project/store`. Endpoints that aren't HTTP or HTTPS URLs are rejected.

`Client::store` sends a multipart request: a CBOR `metadata` part with the key, secondary key, and value as byte strings, and an optional `blob` part streamed from a file. Only HTTP 200 counts as success. The response body isn't decoded.

reqwest configures TLS. It uses the process's default rustls crypto provider, which the client installs as ring unless one is already installed, and verifies certificates with the operating system's verifier. Connections time out after 10 seconds. Reads time out after 60 seconds, and until the response headers arrive, that limit also covers sending the request.

`Error` names the kind of failure: an invalid endpoint, a client that couldn't be created, a blob file that couldn't be read, a network error (including timeouts), or a status other than 200. Its messages contain no OS-specific details, so they can be shown to users as is. The details are in the source: the underlying error, the parse error for an endpoint that isn't a URL, or the message in an error response's body.
