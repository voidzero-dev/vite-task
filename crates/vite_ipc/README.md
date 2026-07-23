# `vite_ipc`

Name-based cross-platform byte transport for communication between a server
and its child processes.

The server exposes an opaque name that can be passed through an environment
variable or process argument. Clients connect synchronously, while the server
accepts connections asynchronously with Tokio.
