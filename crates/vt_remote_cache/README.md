# vt_remote_cache

Client for the [remote cache server API](https://github.com/voidzero-dev/vite-task/pull/713). It treats keys, values, and blobs as opaque bytes. Encoding task cache entries into them is up to the caller.

`Client::new` takes the configured endpoint, which can include a namespace path, such as `https://cache.example.com/projects/my-project`. Each operation appends its route to that path, so a store goes to `https://cache.example.com/projects/my-project/store`. Endpoints that aren't HTTP or HTTPS URLs are rejected. It also takes a `StoreAuth`, which says how stores authenticate. Fetches and downloads never send credentials.

`Client::fetch` sends a key and a secondary key as a CBOR map of byte strings. A 404 response means neither key matched, so `fetch` returns `None` without decoding the body. The body of a 200 response is a CBOR map tagged by `kind`, and `Fetched` decodes only two kinds: `exact`, an exact match with its value and blob ID, and `fallback`, a fallback match with the key it's stored under. Any other kind is a malformed response. An exact match must include `blob_id`, which is null when there's no blob. The fallback's value and blob ID aren't decoded.

`Client::download` gets a blob by its ID. After a 200 response, it returns a `Download`, from which the caller reads the blob's chunks as they arrive.

`Client::store` sends a multipart request: a CBOR `metadata` part with the key, secondary key, and value as byte strings, and an optional `blob` part streamed from a file. The response body isn't decoded.

Only HTTP 200 counts as success for every operation, except that a fetch also accepts 404 as no match. Redirects aren't followed, so a redirect fails like any other status.

reqwest configures TLS. It uses the process's default rustls crypto provider, which the client installs as ring unless one is already installed, and verifies certificates with the operating system's verifier. Requests go through the proxy set in `HTTPS_PROXY`, `HTTP_PROXY`, or `ALL_PROXY`, except for hosts in `NO_PROXY`. Without those variables, the system proxy settings are used on macOS and Windows. Connections time out after 10 seconds. Reads time out after 60 seconds, and until the response headers arrive, that limit also covers sending the request.

`Error` names the kind of failure: an invalid endpoint, a client that couldn't be created, a blob file that couldn't be read, a network error (including timeouts and responses that end early), a status other than 200 (or 404, for a fetch), a malformed fetch response, or a failed OIDC token request. Its messages contain no OS-specific details, so they can be shown to users as is. The details are in the source: the underlying error, the parse error for an endpoint that isn't a URL, or the message in an error response's body. Network errors leave out the request URL, since the endpoint may contain credentials, such as a token in its query.

## GitHub Actions OIDC

With `StoreAuth::GithubOidc`, which holds the values of a GitHub Actions job's `ACTIONS_ID_TOKEN_REQUEST_URL` and `ACTIONS_ID_TOKEN_REQUEST_TOKEN`, each store sends a GitHub OIDC token as `Authorization: Bearer <token>`. GitHub sets these variables only in jobs granted `id-token: write`. Before the first store, the client requests a token with `GET <request URL>&audience=<audience>`, adding `audience` to the URL's query, and `Authorization: Bearer <request token>`. The request token goes only to the token endpoint. The audience is the endpoint without its query, fragment, userinfo, or trailing slash, such as `https://cache.example.com/projects/my-project`. The token endpoint must respond with 200, without a redirect, and a JSON object whose `value` is the token, a JWT.

The client keeps the token in memory and reuses it for later stores until it expires within 60 seconds, reading its `exp` claim without verifying it. Stores that need a new token at the same time wait for a single request. If a token request fails, the client keeps its error and returns it for every later store without requesting again. The error leaves out the token endpoint's URL and response body. If the endpoint has userinfo, the token replaces its basic credentials. `Debug` output leaves out the tokens and the endpoint.

`StoreAuth::GithubActionsWithoutOidc` is for GitHub Actions jobs that lack these variables. Stores send no credentials, like `StoreAuth::Anonymous`, but a 401 response is `Error::MissingIdTokenPermission`, whose message is the same as for any other 401 and whose cause suggests granting `id-token: write`. `Error::is_unauthorized` is true for it, a failed token request, and 401 and 403 responses.
