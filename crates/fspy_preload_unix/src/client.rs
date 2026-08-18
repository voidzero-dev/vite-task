use std::sync::OnceLock;

use convert::{ToAbsolutePath, ToAccessMode};
pub use fspy_client_unix::{Client, convert, raw_exec};

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
        // SAFETY: path and mode contain valid pointers/values forwarded
        // from the interposed function's caller.
        unsafe { client.try_handle_open(path, mode, allocator) }.unwrap();
    }
}

#[cfg(not(test))]
#[ctor::ctor(unsafe)]
fn init_client() {
    // SAFETY: the ctor only reads the process environment while constructing
    // the client and does not retain borrowed environment views.
    let current = unsafe { fspy_nostd::env::current() }.unwrap();
    // The attach's storage: one page-backed bump housed in a static, so
    // its borrow is 'static by construction and the client comes out as
    // Client<'static> with no lifetime promotion anywhere. The bump is not
    // Sync, so this handle cannot be stored globally by any safe code, and
    // the Send/Sync assertion on Client proves the client keeps no handle.
    static BUMP: static_cell::StaticCell<fspy_nostd_alloc::PageBump> =
        static_cell::StaticCell::new();
    let bump: &'static fspy_nostd_alloc::PageBump = BUMP.init(fspy_nostd_alloc::page_bump());
    let client = Client::from_env(current.envs(), bump);
    CLIENT.set(client).unwrap();
}
