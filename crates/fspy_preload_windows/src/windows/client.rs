use std::{
    cell::{RefCell, SyncUnsafeCell},
    ffi::CStr,
    mem::MaybeUninit,
};

use fspy_detours_sys::DetourCopyPayloadToProcess;
use fspy_shared::{
    ipc::{PathAccess, PathAccessSender},
    windows::{PAYLOAD_ID, Payload},
};
use winapi::{shared::minwindef::BOOL, um::winnt::HANDLE};

pub struct Client<'a> {
    payload: Payload<'a>,
}

impl<'a> Client<'a> {
    pub fn from_payload_bytes(payload_bytes: &'a [u8]) -> Self {
        let payload: Payload<'a> = wincode::deserialize_exact(payload_bytes).unwrap();
        Self { payload }
    }

    pub fn send(&self, access: PathAccess<'_>) {
        THREAD_CLIENT.with(|thread_client| {
            // Connecting the pipe triggers the same file APIs fspy detours.
            // A nested report is transport noise, so skip it.
            let Ok(mut thread_client) = thread_client.try_borrow_mut() else {
                return;
            };

            if matches!(&*thread_client, ThreadClient::Uninitialized) {
                let server_name = self.payload.server_name.to_cow_os_str();
                *thread_client = match PathAccessSender::connect(&server_name) {
                    Ok(sender) => ThreadClient::Connected(sender),
                    Err(error) => {
                        report_connection_error(&error);
                        ThreadClient::Disconnected
                    }
                };
            }

            let error = match &mut *thread_client {
                ThreadClient::Connected(sender) => sender.send(access).err(),
                ThreadClient::Uninitialized | ThreadClient::Disconnected => None,
            };
            if let Some(error) = error {
                report_connection_error(&error);
                *thread_client = ThreadClient::Disconnected;
            }
        });
    }

    pub unsafe fn prepare_child_process(&self, child_handle: HANDLE) -> BOOL {
        let payload_bytes = wincode::serialize(&self.payload).unwrap();
        // SAFETY: FFI call to DetourCopyPayloadToProcess with valid handle and payload buffer
        unsafe {
            DetourCopyPayloadToProcess(
                child_handle,
                &PAYLOAD_ID,
                payload_bytes.as_ptr().cast(),
                payload_bytes.len().try_into().unwrap(),
            )
        }
    }

    pub const fn ansi_dll_path(&self) -> &'a CStr {
        // SAFETY: payload.ansi_dll_path_with_nul is guaranteed to be a valid null-terminated byte string
        unsafe { CStr::from_bytes_with_nul_unchecked(self.payload.ansi_dll_path_with_nul) }
    }
}

enum ThreadClient {
    Uninitialized,
    Connected(PathAccessSender),
    Disconnected,
}

thread_local! {
    static THREAD_CLIENT: RefCell<ThreadClient> = const { RefCell::new(ThreadClient::Uninitialized) };
}

#[expect(clippy::print_stderr, reason = "preload library uses stderr for connection diagnostics")]
fn report_connection_error(error: &std::io::Error) {
    eprintln!("fspy: path access connection failed: {error}");
}

static CLIENT: SyncUnsafeCell<MaybeUninit<Client<'static>>> =
    SyncUnsafeCell::new(MaybeUninit::uninit());

pub unsafe fn set_global_client(client: Client<'static>) {
    // SAFETY: called once during DLL_PROCESS_ATTACH before any concurrent access
    unsafe { *CLIENT.get() = MaybeUninit::new(client) }
}

pub unsafe fn global_client() -> &'static Client<'static> {
    // SAFETY: CLIENT is initialized via set_global_client during DLL_PROCESS_ATTACH
    unsafe { (*CLIENT.get()).assume_init_ref() }
}
