# Implementation Plan

1. **Protocol** — `vite_task_ipc_shared`. Define message types and serialization. Everything else depends on this. ✅
2. **Transport** — `vite_task_server` + `vite_task_client`. Build both sides, test them against each other directly in Rust. ✅
3. **Extract artifact** — Pull `artifact.rs` out of fspy into a shared crate. Prerequisite for dylib embedding. ✅
4. **JS bridge** — `vite_task_client_napi` (real impl) + `@voidzero-dev/vite-task-client` (JS wrapper with `getEnvs` logic). ✅
5. **Runner integration** — Wire into `vite_task` spawn: start server per execution, embed/extract dylib, inject the IPC envs via `serve()`'s returned iterator. ✅
6. **Cache integration** — Runner consumes the reported data (ignored inputs/outputs, requested envs, disable cache) and adjusts caching behavior. ✅
