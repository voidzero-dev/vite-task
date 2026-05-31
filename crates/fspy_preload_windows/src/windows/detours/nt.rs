use fspy_shared::ipc::{AccessMode, NativePath, PathAccess};
use ntapi::ntioapi::{
    FILE_INFORMATION_CLASS, NtQueryDirectoryFile, NtQueryFullAttributesFile,
    NtQueryInformationByName, PFILE_BASIC_INFORMATION, PFILE_NETWORK_OPEN_INFORMATION,
    PIO_APC_ROUTINE, PIO_STATUS_BLOCK,
};
use winapi::{
    shared::{
        minwindef::HFILE,
        ntdef::{
            BOOLEAN, HANDLE, NTSTATUS, PHANDLE, PLARGE_INTEGER, POBJECT_ATTRIBUTES,
            PUNICODE_STRING, PVOID, ULONG,
        },
    },
    um::winnt::{ACCESS_MASK, GENERIC_READ},
};

use crate::windows::{
    client::global_client,
    convert::{ToAbsolutePath, ToAccessMode},
    detour::{Detour, DetourAny},
    winapi_utils::{access_mask_to_mode, get_path_name},
};

static DETOUR_NT_CREATE_FILE: Detour<
    unsafe extern "system" fn(
        file_handle: PHANDLE,
        desired_access: ACCESS_MASK,
        object_attributes: POBJECT_ATTRIBUTES,
        io_status_block: PIO_STATUS_BLOCK,
        allocation_size: PLARGE_INTEGER,
        file_attributes: ULONG,
        share_access: ULONG,
        create_disposition: ULONG,
        create_options: ULONG,
        ea_buffer: PVOID,
        ea_length: ULONG,
    ) -> HFILE,
> =
    // SAFETY: initializing Detour with the real NtCreateFile function pointer and our replacement
    unsafe {
        Detour::new(c"NtCreateFile", ntapi::ntioapi::NtCreateFile, {
            unsafe extern "system" fn new_nt_create_file(
                file_handle: PHANDLE,
                desired_access: ACCESS_MASK,
                object_attributes: POBJECT_ATTRIBUTES,
                io_status_block: PIO_STATUS_BLOCK,
                allocation_size: PLARGE_INTEGER,
                file_attributes: ULONG,
                share_access: ULONG,
                create_disposition: ULONG,
                create_options: ULONG,
                ea_buffer: PVOID,
                ea_length: ULONG,
            ) -> HFILE {
                // SAFETY: intercepting file open to record access before forwarding to real function
                unsafe { handle_open(desired_access, object_attributes) };

                // SAFETY: calling the original NtCreateFile with all original arguments
                let status = unsafe {
                    (DETOUR_NT_CREATE_FILE.real())(
                        file_handle,
                        desired_access,
                        object_attributes,
                        io_status_block,
                        allocation_size,
                        file_attributes,
                        share_access,
                        create_disposition,
                        create_options,
                        ea_buffer,
                        ea_length,
                    )
                };
                if status >= 0 {
                    // SAFETY: the open succeeded, so `file_handle` points to a valid handle.
                    unsafe { handle_open_callback(desired_access, file_handle) };
                }
                status
            }
            new_nt_create_file
        })
    };

static DETOUR_NT_OPEN_FILE: Detour<
    unsafe extern "system" fn(
        file_handle: PHANDLE,
        desired_access: ACCESS_MASK,
        object_attributes: POBJECT_ATTRIBUTES,
        io_status_block: PIO_STATUS_BLOCK,
        share_access: ULONG,
        open_options: ULONG,
    ) -> HFILE,
> =
    // SAFETY: initializing Detour with the real NtOpenFile function pointer and our replacement
    unsafe {
        Detour::new(c"NtOpenFile", ntapi::ntioapi::NtOpenFile, {
            unsafe extern "system" fn new_nt_open_file(
                file_handle: PHANDLE,
                desired_access: ACCESS_MASK,
                object_attributes: POBJECT_ATTRIBUTES,
                io_status_block: PIO_STATUS_BLOCK,
                share_access: ULONG,
                open_options: ULONG,
            ) -> HFILE {
                // SAFETY: intercepting file open to record access before forwarding to real function
                unsafe {
                    handle_open(desired_access, object_attributes);
                }

                // SAFETY: calling the original NtOpenFile with all original arguments
                let status = unsafe {
                    (DETOUR_NT_OPEN_FILE.real())(
                        file_handle,
                        desired_access,
                        object_attributes,
                        io_status_block,
                        share_access,
                        open_options,
                    )
                };
                if status >= 0 {
                    // SAFETY: the open succeeded, so `file_handle` points to a valid handle.
                    unsafe { handle_open_callback(desired_access, file_handle) };
                }
                status
            }
            new_nt_open_file
        })
    };

static DETOUR_NT_QUERY_ATTRIBUTES_FILE: Detour<
    unsafe extern "system" fn(
        object_attributes: POBJECT_ATTRIBUTES,
        file_information: PFILE_BASIC_INFORMATION,
    ) -> HFILE,
> =
    // SAFETY: initializing Detour with the real NtQueryAttributesFile function pointer and our replacement
    unsafe {
        Detour::new(c"NtQueryAttributesFile", ntapi::ntioapi::NtQueryAttributesFile, {
            unsafe extern "system" fn new_nt_query_attrs(
                object_attributes: POBJECT_ATTRIBUTES,
                file_information: PFILE_BASIC_INFORMATION,
            ) -> HFILE {
                // SAFETY: intercepting attribute query to record read access
                unsafe { handle_open(AccessMode::READ, object_attributes) };
                // SAFETY: calling the original NtQueryAttributesFile with all original arguments
                unsafe {
                    (DETOUR_NT_QUERY_ATTRIBUTES_FILE.real())(object_attributes, file_information)
                }
            }
            new_nt_query_attrs
        })
    };

unsafe fn handle_open(access_mode: impl ToAccessMode, path: impl ToAbsolutePath) {
    // SAFETY: accessing the global client which was initialized during DLL_PROCESS_ATTACH
    let client = unsafe { global_client() };
    // SAFETY: resolving path from Windows object attributes or handle for access tracking
    unsafe {
        path.to_absolute_path(|path| {
            let Some(path) = path else {
                return Ok(());
            };
            let path = path.as_slice();
            let path_access = path.iter().rposition(|c| *c == u16::from(b'*')).map_or_else(
                || {
                    // SAFETY: converting access mask to AccessMode via FFI-aware trait
                    PathAccess {
                        mode: access_mode.to_access_mode(),
                        path: NativePath::from_wide(path),
                    }
                },
                |wildcard_pos| {
                    let path_before_wildcard = &path[..wildcard_pos];
                    let slash_pos = path_before_wildcard
                        .iter()
                        .rposition(|c| *c == u16::from(b'\\') || *c == u16::from(b'/'))
                        .unwrap_or(0);
                    PathAccess {
                        mode: AccessMode::READ_DIR,
                        path: NativePath::from_wide(&path[..slash_pos]),
                    }
                },
            );
            client.send(path_access);
            Ok(())
        })
    }
    .unwrap();
}

static DETOUR_NT_FULL_QUERY_ATTRIBUTES_FILE: Detour<
    unsafe extern "system" fn(
        object_attributes: POBJECT_ATTRIBUTES,
        file_information: PFILE_NETWORK_OPEN_INFORMATION,
    ) -> HFILE,
> =
    // SAFETY: initializing Detour with the real NtQueryFullAttributesFile function pointer
    unsafe {
        Detour::new(c"NtQueryFullAttributesFile", NtQueryFullAttributesFile, {
            unsafe extern "system" fn new_fn(
                object_attributes: POBJECT_ATTRIBUTES,
                file_information: PFILE_NETWORK_OPEN_INFORMATION,
            ) -> HFILE {
                // SAFETY: intercepting attribute query to record read access
                unsafe { handle_open(GENERIC_READ, object_attributes) };
                // SAFETY: calling the original NtQueryFullAttributesFile
                unsafe {
                    (DETOUR_NT_FULL_QUERY_ATTRIBUTES_FILE.real())(
                        object_attributes,
                        file_information,
                    )
                }
            }
            new_fn
        })
    };

static DETOUR_NT_OPEN_SYMBOLIC_LINK_OBJECT: Detour<
    unsafe extern "system" fn(
        link_handle: PHANDLE,
        desired_access: ACCESS_MASK,
        object_attributes: POBJECT_ATTRIBUTES,
    ) -> HFILE,
> =
    // SAFETY: initializing Detour with the real NtOpenSymbolicLinkObject function pointer
    unsafe {
        Detour::new(c"NtOpenSymbolicLinkObject", ntapi::ntobapi::NtOpenSymbolicLinkObject, {
            unsafe extern "system" fn new_fn(
                link_handle: PHANDLE,
                desired_access: ACCESS_MASK,
                object_attributes: POBJECT_ATTRIBUTES,
            ) -> HFILE {
                // SAFETY: intercepting symlink open to record access
                unsafe { handle_open(desired_access, object_attributes) };
                // SAFETY: calling the original NtOpenSymbolicLinkObject
                unsafe {
                    (DETOUR_NT_OPEN_SYMBOLIC_LINK_OBJECT.real())(
                        link_handle,
                        desired_access,
                        object_attributes,
                    )
                }
            }
            new_fn
        })
    };

static DETOUR_NT_QUERY_INFORMATION_BY_NAME: Detour<
    unsafe extern "system" fn(
        object_attributes: POBJECT_ATTRIBUTES,
        io_status_block: PIO_STATUS_BLOCK,
        file_information: PVOID,
        length: ULONG,
        file_information_class: FILE_INFORMATION_CLASS,
    ) -> HFILE,
> =
    // SAFETY: initializing Detour with the real NtQueryInformationByName function pointer
    unsafe {
        Detour::new(c"NtQueryInformationByName", NtQueryInformationByName, {
            unsafe extern "system" fn new_fn(
                object_attributes: POBJECT_ATTRIBUTES,
                io_status_block: PIO_STATUS_BLOCK,
                file_information: PVOID,
                length: ULONG,
                file_information_class: FILE_INFORMATION_CLASS,
            ) -> HFILE {
                // SAFETY: intercepting information query to record read access
                unsafe { handle_open(GENERIC_READ, object_attributes) };
                // SAFETY: calling the original NtQueryInformationByName
                unsafe {
                    (DETOUR_NT_QUERY_INFORMATION_BY_NAME.real())(
                        object_attributes,
                        io_status_block,
                        file_information,
                        length,
                        file_information_class,
                    )
                }
            }
            new_fn
        })
    };

static DETOUR_NT_QUERY_DIRECTORY_FILE: Detour<
    unsafe extern "system" fn(
        file_handle: HANDLE,
        event: HANDLE,
        apc_routine: PIO_APC_ROUTINE,
        apc_context: PVOID,
        io_status_block: PIO_STATUS_BLOCK,
        file_information: PVOID,
        length: ULONG,
        file_information_class: FILE_INFORMATION_CLASS,
        return_single_entry: BOOLEAN,
        file_name: PUNICODE_STRING,
        restart_scan: BOOLEAN,
    ) -> NTSTATUS,
> =
    // SAFETY: initializing Detour with the real NtQueryDirectoryFile function pointer
    unsafe {
        Detour::new(c"NtQueryDirectoryFile", NtQueryDirectoryFile, {
            unsafe extern "system" fn new_fn(
                file_handle: HANDLE,
                event: HANDLE,
                apc_routine: PIO_APC_ROUTINE,
                apc_context: PVOID,
                io_status_block: PIO_STATUS_BLOCK,
                file_information: PVOID,
                length: ULONG,
                file_information_class: FILE_INFORMATION_CLASS,
                return_single_entry: BOOLEAN,
                file_name: PUNICODE_STRING,
                restart_scan: BOOLEAN,
            ) -> NTSTATUS {
                // SAFETY: intercepting directory query to record directory read access
                unsafe { handle_open(AccessMode::READ_DIR, file_handle) };
                // SAFETY: calling the original NtQueryDirectoryFile
                unsafe {
                    (DETOUR_NT_QUERY_DIRECTORY_FILE.real())(
                        file_handle,
                        event,
                        apc_routine,
                        apc_context,
                        io_status_block,
                        file_information,
                        length,
                        file_information_class,
                        return_single_entry,
                        file_name,
                        restart_scan,
                    )
                }
            }
            new_fn
        })
    };

// NtQueryDirectoryFileEx is not in ntapi crate, so we define it here.
// https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/nf-ntifs-ntquerydirectoryfileex
type NtQueryDirectoryFileExFn = unsafe extern "system" fn(
    file_handle: HANDLE,
    event: HANDLE,
    apc_routine: PIO_APC_ROUTINE,
    apc_context: PVOID,
    io_status_block: PIO_STATUS_BLOCK,
    file_information: PVOID,
    length: ULONG,
    file_information_class: FILE_INFORMATION_CLASS,
    query_flags: ULONG,
    file_name: PUNICODE_STRING,
) -> NTSTATUS;

static DETOUR_NT_QUERY_DIRECTORY_FILE_EX: Detour<NtQueryDirectoryFileExFn> =
    // SAFETY: initializing dynamic Detour for NtQueryDirectoryFileEx (resolved at attach time)
    unsafe {
        Detour::dynamic(c"NtQueryDirectoryFileEx", {
            unsafe extern "system" fn new_fn(
                file_handle: HANDLE,
                event: HANDLE,
                apc_routine: PIO_APC_ROUTINE,
                apc_context: PVOID,
                io_status_block: PIO_STATUS_BLOCK,
                file_information: PVOID,
                length: ULONG,
                file_information_class: FILE_INFORMATION_CLASS,
                query_flags: ULONG,
                file_name: PUNICODE_STRING,
            ) -> NTSTATUS {
                // SAFETY: intercepting directory query to record directory read access
                unsafe { handle_open(AccessMode::READ_DIR, file_handle) };
                // SAFETY: calling the original NtQueryDirectoryFileEx
                unsafe {
                    (DETOUR_NT_QUERY_DIRECTORY_FILE_EX.real())(
                        file_handle,
                        event,
                        apc_routine,
                        apc_context,
                        io_status_block,
                        file_information,
                        length,
                        file_information_class,
                        query_flags,
                        file_name,
                    )
                }
            }
            new_fn
        })
    };

/// Runs the post-open blocking callback, if one is registered, for the handle
/// produced by a successful `NtCreateFile`/`NtOpenFile`.
unsafe fn handle_open_callback(desired_access: ACCESS_MASK, file_handle: PHANDLE) {
    // SAFETY: the global client is initialized during DLL_PROCESS_ATTACH.
    let Some(callback) = (unsafe { global_client() }).callback() else {
        return;
    };
    // SAFETY: the open succeeded, so `file_handle` points to a valid handle.
    let handle = unsafe { *file_handle };
    // SAFETY: `handle` is a valid file handle just produced by the open.
    let Ok(path) = (unsafe { get_path_name(handle) }) else {
        return;
    };
    callback.run_open_callback(handle, access_mask_to_mode(desired_access), &path);
}

/// Runs the pre-close blocking callback, if one is registered, for a handle
/// about to be closed.
unsafe fn handle_close(handle: HANDLE) {
    // SAFETY: the global client is initialized during DLL_PROCESS_ATTACH.
    if let Some(callback) = (unsafe { global_client() }).callback() {
        callback.run_close_callback(handle);
    }
}

type NtCloseFn = unsafe extern "system" fn(handle: HANDLE) -> NTSTATUS;

static DETOUR_NT_CLOSE: Detour<NtCloseFn> =
    // SAFETY: initializing dynamic Detour for NtClose (resolved at attach time)
    unsafe {
        Detour::dynamic(c"NtClose", {
            unsafe extern "system" fn new_nt_close(handle: HANDLE) -> NTSTATUS {
                // SAFETY: the pre-close callback runs while `handle` is still valid.
                unsafe { handle_close(handle) };
                // SAFETY: calling the original NtClose with the original argument.
                unsafe { (DETOUR_NT_CLOSE.real())(handle) }
            }
            new_nt_close
        })
    };

pub const DETOURS: &[DetourAny] = &[
    DETOUR_NT_CREATE_FILE.as_any(),
    DETOUR_NT_OPEN_FILE.as_any(),
    DETOUR_NT_QUERY_ATTRIBUTES_FILE.as_any(),
    DETOUR_NT_FULL_QUERY_ATTRIBUTES_FILE.as_any(),
    DETOUR_NT_OPEN_SYMBOLIC_LINK_OBJECT.as_any(),
    DETOUR_NT_QUERY_INFORMATION_BY_NAME.as_any(),
    DETOUR_NT_QUERY_DIRECTORY_FILE.as_any(),
    DETOUR_NT_QUERY_DIRECTORY_FILE_EX.as_any(),
    DETOUR_NT_CLOSE.as_any(),
];
