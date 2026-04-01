# Playground

A workspace for manually testing `cargo run --bin vt run ...`.

## Structure

```
playground/
├── packages/
│   ├── app/       → depends on @playground/lib
│   ├── lib/       → depends on @playground/utils
│   └── utils/     → no dependencies
└── vite-task.json → workspace-level task config
```

Dependency chain: `app → lib → utils`

## Scripts & Tasks

Tasks are defined in each package's `vite-task.json` with caching enabled. `dev` is a package.json script (not cached).

| Name        | Type   | Packages        | Cached | Description                                    |
| ----------- | ------ | --------------- | ------ | ---------------------------------------------- |
| `build`     | task   | app, lib, utils | yes    | Prints a build message                         |
| `test`      | task   | app, lib, utils | yes    | Prints a test message                          |
| `lint`      | task   | app, lib, utils | yes    | Prints a lint message                          |
| `typecheck` | task   | app, lib        | yes    | Prints a typecheck message                     |
| `dev`       | script | app, lib        | no     | Long-running process (prints every 2s, ctrl-c) |
