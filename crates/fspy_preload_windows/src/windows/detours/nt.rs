use std::mem::{offset_of, size_of};

use fspy_shared::ipc::{AccessMode, NativePath, PathAccess};
use ntapi::{
    ntioapi::{
        FILE_INFORMATION_CLASS, NtQueryDirectoryFile, NtQueryFullAttributesFile,
        NtQueryInformationByName, PFILE_BASIC_INFORMATION, PFILE_NETWORK_OPEN_INFORMATION,
        PIO_APC_ROUTINE, PIO_STATUS_BLOCK,
    },
    ntpsapi::{
        NtCreateUserProcess, PPS_ATTRIBUTE_LIST, PPS_CREATE_INFO, PS_ATTRIBUTE,
        PS_ATTRIBUTE_IMAGE_NAME, PS_ATTRIBUTE_LIST,
    },
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
};

// CreateProcess ultimately asks NtCreateUserProcess to open the executable image. Some Windows
// versions perform that open entirely inside the syscall, so it never reaches the NtCreateFile and
// query functions hooked below. PS_ATTRIBUTE_IMAGE_NAME is the kernel-facing path that identifies
// the image to open; RTL_USER_PROCESS_PARAMETERS.ImagePathName is only metadata for the child PEB
// and can intentionally name a different file.
//
// Record the image before forwarding the syscall, matching the attempted-access semantics of the
// other NT hooks in this module. This is important for missing executables: the failed lookup is
// still an input access even though NtCreateUserProcess returns an error.
static DETOUR_NT_CREATE_USER_PROCESS: Detour<
    unsafe extern "system" fn(
        process_handle: PHANDLE,
        thread_handle: PHANDLE,
        process_desired_access: ACCESS_MASK,
        thread_desired_access: ACCESS_MASK,
        process_object_attributes: POBJECT_ATTRIBUTES,
        thread_object_attributes: POBJECT_ATTRIBUTES,
        process_flags: ULONG,
        thread_flags: ULONG,
        process_parameters: PVOID,
        create_info: PPS_CREATE_INFO,
        attribute_list: PPS_ATTRIBUTE_LIST,
    ) -> NTSTATUS,
> =
    // SAFETY: initializing Detour with the real NtCreateUserProcess function pointer
    unsafe {
        Detour::new(c"NtCreateUserProcess", NtCreateUserProcess, {
            unsafe extern "system" fn new_fn(
                process_handle: PHANDLE,
                thread_handle: PHANDLE,
                process_desired_access: ACCESS_MASK,
                thread_desired_access: ACCESS_MASK,
                process_object_attributes: POBJECT_ATTRIBUTES,
                thread_object_attributes: POBJECT_ATTRIBUTES,
                process_flags: ULONG,
                thread_flags: ULONG,
                process_parameters: PVOID,
                create_info: PPS_CREATE_INFO,
                attribute_list: PPS_ATTRIBUTE_LIST,
            ) -> NTSTATUS {
                // SAFETY: observing caller memory without changing the forwarded arguments
                unsafe { handle_process_image(attribute_list) };

                // SAFETY: calling the original NtCreateUserProcess with all original arguments
                unsafe {
                    (DETOUR_NT_CREATE_USER_PROCESS.real())(
                        process_handle,
                        thread_handle,
                        process_desired_access,
                        thread_desired_access,
                        process_object_attributes,
                        thread_object_attributes,
                        process_flags,
                        thread_flags,
                        process_parameters,
                        create_info,
                        attribute_list,
                    )
                }
            }
            new_fn
        })
    };

unsafe fn handle_process_image(attribute_list: PPS_ATTRIBUTE_LIST) {
    // SAFETY: NtCreateUserProcess requires its attribute list to remain valid for this call.
    if let Some(image_path) = unsafe { read_process_image_attribute(attribute_list) } {
        // Sender serialization completes before this call returns, so NativePath does not retain
        // the borrowed PS_ATTRIBUTE_IMAGE_NAME buffer past the NtCreateUserProcess call.
        // SAFETY: accessing the global client which was initialized during DLL_PROCESS_ATTACH
        unsafe { global_client() }
            .send(PathAccess { mode: AccessMode::READ, path: NativePath::from_wide(image_path) });
    }
}

/// Find the kernel-facing executable name in an `NtCreateUserProcess` attribute list.
///
/// `PS_ATTRIBUTE_LIST` is a variable-length structure: `TotalLength` covers a fixed-size header
/// followed by contiguous `PS_ATTRIBUTE` entries. Its Rust definition contains one placeholder
/// element, so the actual entry count must be derived from `TotalLength`, not from the array type.
///
/// The returned slice borrows the caller's `PS_ATTRIBUTE_IMAGE_NAME` buffer and is valid only while
/// the intercepted `NtCreateUserProcess` call is active.
///
/// # Safety
///
/// `attribute_list` and the image-name buffer it references must remain valid for the duration of
/// the intercepted call, as required by `NtCreateUserProcess`.
unsafe fn read_process_image_attribute<'a>(
    attribute_list: PPS_ATTRIBUTE_LIST,
) -> Option<&'a [u16]> {
    // SAFETY: NtCreateUserProcess keeps a supplied attribute list valid for this call; a null
    // optional pointer is parsed as None.
    let attribute_list = unsafe { attribute_list.as_ref()? };

    // Attributes is a trailing array. Subtract its byte offset to remove the header; the native API
    // contract guarantees that the remaining bytes contain complete PS_ATTRIBUTE entries.
    let attributes_offset = offset_of!(PS_ATTRIBUTE_LIST, Attributes);
    let attribute_count =
        (attribute_list.TotalLength - attributes_offset) / size_of::<PS_ATTRIBUTE>();

    // The Rust field exposes the first placeholder entry as a reference; TotalLength describes how
    // many contiguous entries follow it in the actual variable-length allocation.
    let first_attribute = attribute_list.Attributes.first()?;
    // SAFETY: TotalLength covers a contiguous variable-length tail starting at first_attribute.
    let attributes: &[PS_ATTRIBUTE] =
        unsafe { std::slice::from_raw_parts(std::ptr::from_ref(first_attribute), attribute_count) };
    for attribute in attributes {
        if attribute.Attribute != PS_ATTRIBUTE_IMAGE_NAME {
            continue;
        }

        // Unlike a UNICODE_STRING, PS_ATTRIBUTE_IMAGE_NAME stores the path buffer directly in
        // ValuePtr and stores its byte length in Size. It is the image path consumed by the kernel,
        // so do not fall back to the separately spoofable process-parameter path.
        // SAFETY: PS_ATTRIBUTE_IMAGE_NAME stores a valid UTF-16 pointer in ValuePtr for this call;
        // a null pointer is parsed as None.
        let image_path = unsafe { attribute.u.ValuePtr.cast::<u16>().as_ref()? };
        // SAFETY: the attribute contract guarantees a valid UTF-16 buffer of Size bytes for this
        // call. Size is the counted string length, so no NUL-terminator parsing is needed.
        return Some(unsafe {
            std::slice::from_raw_parts(
                std::ptr::from_ref(image_path),
                attribute.Size / size_of::<u16>(),
            )
        });
    }

    None
}

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
                unsafe {
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
                }
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
                unsafe {
                    (DETOUR_NT_OPEN_FILE.real())(
                        file_handle,
                        desired_access,
                        object_attributes,
                        io_status_block,
                        share_access,
                        open_options,
                    )
                }
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

pub const DETOURS: &[DetourAny] = &[
    DETOUR_NT_CREATE_USER_PROCESS.as_any(),
    DETOUR_NT_CREATE_FILE.as_any(),
    DETOUR_NT_OPEN_FILE.as_any(),
    DETOUR_NT_QUERY_ATTRIBUTES_FILE.as_any(),
    DETOUR_NT_FULL_QUERY_ATTRIBUTES_FILE.as_any(),
    DETOUR_NT_OPEN_SYMBOLIC_LINK_OBJECT.as_any(),
    DETOUR_NT_QUERY_INFORMATION_BY_NAME.as_any(),
    DETOUR_NT_QUERY_DIRECTORY_FILE.as_any(),
    DETOUR_NT_QUERY_DIRECTORY_FILE_EX.as_any(),
];
