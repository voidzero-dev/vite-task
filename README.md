# Vite Task

Monorepo task runner with intelligent caching and dependency-aware scheduling, powering [`vp run`](https://github.com/voidzero-dev/vite-plus) in [Vite+](https://viteplus.dev).

## Getting Started

Install [Vite+](https://viteplus.dev), then run tasks from your workspace. See the [documentation](https://viteplus.dev/guide/run) for full usage.

```bash
vp run build              # run a task in the current package
vp run -r build           # run across all packages in dependency order
vp run -t @my/app#build   # run in a package and its transitive dependencies
vp run --cache build      # run with caching enabled
```

## Remote cache service

[![Deploy to Cloudflare](https://deploy.workers.cloudflare.com/button)](https://deploy.workers.cloudflare.com/?url=https%3A%2F%2Fgithub.com%2Fvoidzero-dev%2Fvite-task%2Ftree%2Ffeat%2Fpublic-remote-cache%2Fpackages%2Fremote-cache)

To deploy the public remote cache service in your own Cloudflare account and bind it to a GitHub repository, follow the [self-hosting guide](packages/remote-cache/docs/self-hosting.md). The service is available in this source tree; the `vp run` remote-cache client adapter is not yet implemented here.

## Sponsors

Thanks to [namespace.so](https://namespace.so) for powering our CI/CD pipelines with fast, free macOS and Linux runners.

## License

[MIT](LICENSE)

Copyright (c) 2026-present [VoidZero Inc.](https://voidzero.dev/)
