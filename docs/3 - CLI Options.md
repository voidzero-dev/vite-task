# CLI Options

- `vite run <task_name>`: Executes the specified task defined in `vite-task.json` or `package.json` in the current package.
- `vite run <package_name>#<task_name>`: Executes the specified task in the given package within a monorepo.
- `vite run -r <task_name>`: Executes the task with the specified name across all packages in a monorepo, in topological order based on dependencies.
