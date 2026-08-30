use std::sync::OnceLock;

use convert::{ToAbsolutePath, ToAccessMode};
pub use fspy_client_unix::{Client, ExecInjectionError, convert, raw_exec};

static CLIENT: OnceLock<Client<'static>> = OnceLock::new();

pub fn global_client() -> Option<&'static Client<'static>> {
    CLIENT.get()
}

// The handler needs no re-entry guard: on Linux everything it calls is a
// raw syscall — nothing binds through the PLT, where LD_PRELOAD would
// resolve to our own interposers — and on macOS dyld never applies
// `__interpose` tuples to the interposing image's own bindings, the same
// exemption every `original()` forward relies on. Keep the handler free of
// bindable libc calls on Linux: a call that binds to an interposer here
// recurses until the traced process overflows its stack.
pub unsafe fn handle_open(path: impl ToAbsolutePath, mode: impl ToAccessMode) {
    if let Some(client) = global_client() {
        let allocator = fspy_nostd_alloc::pooled_bump();
        // The interception proceeds whether or not the record could be
        // sent — a preload library can never panic its host process.
        // SAFETY: path and mode contain valid pointers/values forwarded
        // from the interposed function's caller.
        let _ = unsafe { client.try_handle_open(path, mode, allocator) };
    }
}

#[cfg(not(test))]
#[ctor::ctor(unsafe)]
fn init_client() {
    // Never panic here: a panic in a preload constructor aborts the host
    // process. When the environment cannot be read or carries no valid
    // payload (e.g. a leaked LD_PRELOAD in an env-scrubbed sandbox), CLIENT
    // stays unset and the process runs untracked: the interposed calls
    // forward to the originals untouched.
    static BUMP: static_cell::StaticCell<fspy_nostd_alloc::PageBump> =
        static_cell::StaticCell::new();
    // The attach's storage: one page-backed bump housed in a static, so
    // its borrow is 'static by construction and the client comes out as
    // Client<'static> with no lifetime promotion anywhere. The bump is not
    // Sync, so this handle cannot be stored globally by any safe code, and
    // the Send/Sync assertion on Client proves the client keeps no handle.
    let bump: &'static fspy_nostd_alloc::PageBump = BUMP.init(fspy_nostd_alloc::page_bump());
    // SAFETY: the ctor only reads the process environment while constructing
    // the client and does not retain borrowed environment views.
    let client = unsafe { fspy_nostd::env::current() }
        .ok()
        .and_then(|current| Client::from_env(current.envs(), bump));
    if let Some(client) = client {
        let _ = CLIENT.set(client);
    }
}
