# `socket_ipc`

Socket-style server-client IPC, implemented on named pipes rather than Unix
domain sockets.

A server binds under an opaque name and hands that name to its clients, for
example through an environment variable. Clients connect synchronously and get
a byte stream; the server accepts each connection asynchronously with Tokio.
The API is four items: `Server::bind`, `Server::name`, `Server::accept`, and
`Client::connect`.

The transport is named pipes on every platform: FIFOs on Unix, named pipe
objects on Windows. Both are available in environments that block Unix domain
sockets, such as coding-agent sandboxes.

A client whose server is gone gets an error, not a hang, on both platforms.
