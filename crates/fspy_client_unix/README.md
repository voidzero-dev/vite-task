# fspy_client_unix

The Unix client that resolves and reports file accesses to the fspy supervisor.

The preload library owns interception-specific initialization and re-entry
guards. This crate owns the reusable client, path conversion, and exec
transformation so another Unix injection mechanism can provide its own runtime
integration around the same client.
