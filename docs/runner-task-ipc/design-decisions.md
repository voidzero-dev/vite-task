## Why change code in tools instead of configuring in vite-plus

- logic locality
- dynamic decision at runtime
- provide api to tools' plugins.

## Why implement client in rust (instead of pure js)

- Consumable by both rust and js (via napi)
- Easier to implement sync api

## Why provide client at runtime (instead of bundling in the tools)

- Makes IPC protocol a implementation detail. Allows us to evolve IPC implementation or data schema without breaking clients (as long as we maintain the client API contract)
- Easier for 3rd party client implementation in other languages (for example, esbuild can create a golang wrapper over the client ffi)
