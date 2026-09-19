# vt_client

IPC client that connects from tool processes to the task runner to report inputs/outputs, request env values, disable caching, and report unchanged outputs.

`Client::report_unchanged()` waits for acknowledgement, then the runner accepts
it only after the current command exits successfully without cancellation.
Repeated reports are idempotent. Dependent commands still validate their own
cache entries. `Client::from_envs` returns `Ok(None)` outside the runner.
