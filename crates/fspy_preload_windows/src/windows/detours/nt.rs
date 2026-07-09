use std::{
    ffi::c_void,
    mem::{MaybeUninit, offset_of, size_of},
};

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
    ntrtl::{RTL_USER_PROC_PARAMS_NORMALIZED, RTL_USER_PROCESS_PARAMETERS},
};
use winapi::{
    shared::{
        minwindef::HFILE,
        ntdef::{
            BOOLEAN, HANDLE, NTSTATUS, PHANDLE, PLARGE_INTEGER, POBJECT_ATTRIBUTES,
            PUNICODE_STRING, PVOID, ULONG, UNICODE_STRING,
        },
    },
    um::{
        memoryapi::ReadProcessMemory,
        processthreadsapi::GetCurrentProcess,
        winnt::{ACCESS_MASK, GENERIC_READ},
    },
};

use crate::windows::{
    client::global_client,
    convert::{ToAbsolutePath, ToAccessMode},
    detour::{Detour, DetourAny},
};

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
                unsafe { handle_process_image(attribute_list, process_parameters) };

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

unsafe fn handle_process_image(attribute_list: PPS_ATTRIBUTE_LIST, process_parameters: PVOID) {
    // SAFETY: invalid caller-provided memory is handled by ReadProcessMemory
    match unsafe { read_process_image_attribute(attribute_list) } {
        Ok(Some(image_path)) => {
            send_process_image(&image_path);
            return;
        }
        Ok(None) => {}
        Err(()) => return,
    }

    // SAFETY: invalid caller-provided memory is handled by ReadProcessMemory
    let Some(image_path) = (unsafe { read_process_parameters_image(process_parameters) }) else {
        return;
    };
    send_process_image(&image_path);
}

unsafe fn read_process_image_attribute(
    attribute_list: PPS_ATTRIBUTE_LIST,
) -> Result<Option<Vec<u16>>, ()> {
    const MAX_ATTRIBUTE_COUNT: usize = 64;

    if attribute_list.is_null() {
        return Ok(None);
    }

    let attribute_list = attribute_list.cast::<u8>();
    let total_length_address = attribute_list
        .wrapping_add(offset_of!(PS_ATTRIBUTE_LIST, TotalLength))
        .cast_const()
        .cast::<c_void>();
    // SAFETY: invalid caller-provided memory is handled by ReadProcessMemory
    let total_length =
        unsafe { read_current_process_value::<usize>(total_length_address) }.ok_or(())?;
    let attributes_offset = offset_of!(PS_ATTRIBUTE_LIST, Attributes);
    let attributes_length = total_length.checked_sub(attributes_offset).ok_or(())?;
    if attributes_length % size_of::<PS_ATTRIBUTE>() != 0 {
        return Err(());
    }

    let attribute_count = attributes_length / size_of::<PS_ATTRIBUTE>();
    if attribute_count > MAX_ATTRIBUTE_COUNT {
        return Err(());
    }

    for index in 0..attribute_count {
        let attribute_offset = attributes_offset + index * size_of::<PS_ATTRIBUTE>();
        let attribute_address =
            attribute_list.wrapping_add(attribute_offset).cast_const().cast::<c_void>();
        // SAFETY: invalid caller-provided memory is handled by ReadProcessMemory
        let attribute =
            unsafe { read_current_process_value::<PS_ATTRIBUTE>(attribute_address) }.ok_or(())?;
        if attribute.Attribute != PS_ATTRIBUTE_IMAGE_NAME {
            continue;
        }

        // SAFETY: PS_ATTRIBUTE_IMAGE_NAME stores its counted string in ValuePtr
        let image_path = unsafe { attribute.u.ValuePtr };
        // SAFETY: invalid caller-provided memory is handled by ReadProcessMemory
        return unsafe {
            read_current_process_wide_string(image_path.cast_const(), attribute.Size)
        }
        .map(Some)
        .ok_or(());
    }

    Ok(None)
}

unsafe fn read_process_parameters_image(process_parameters: PVOID) -> Option<Vec<u16>> {
    if process_parameters.is_null() {
        return None;
    }

    let parameters = process_parameters.cast::<u8>();
    let flags_address = parameters
        .wrapping_add(offset_of!(RTL_USER_PROCESS_PARAMETERS, Flags))
        .cast_const()
        .cast::<c_void>();
    // SAFETY: invalid caller-provided memory is handled by ReadProcessMemory
    let flags = (unsafe { read_current_process_value::<ULONG>(flags_address) })?;
    if flags & RTL_USER_PROC_PARAMS_NORMALIZED == 0 {
        return None;
    }

    let image_path_address = parameters
        .wrapping_add(offset_of!(RTL_USER_PROCESS_PARAMETERS, ImagePathName))
        .cast_const()
        .cast::<c_void>();
    // SAFETY: invalid caller-provided memory is handled by ReadProcessMemory
    let image_path = (unsafe { read_current_process_value::<UNICODE_STRING>(image_path_address) })?;
    if image_path.Length == 0
        || image_path.Buffer.is_null()
        || image_path.Length % 2 != 0
        || image_path.Length > image_path.MaximumLength
    {
        return None;
    }

    // SAFETY: invalid caller-provided memory is handled by ReadProcessMemory
    unsafe {
        read_current_process_wide_string(
            image_path.Buffer.cast_const().cast(),
            usize::from(image_path.Length),
        )
    }
}

unsafe fn read_current_process_wide_string(
    address: *const c_void,
    byte_len: usize,
) -> Option<Vec<u16>> {
    if address.is_null()
        || byte_len == 0
        || byte_len > usize::from(u16::MAX)
        || !byte_len.is_multiple_of(size_of::<u16>())
    {
        return None;
    }

    let image_path_len = byte_len / size_of::<u16>();
    let mut image_path = Vec::<u16>::with_capacity(image_path_len);
    let mut bytes_read = 0;
    // SAFETY: the destination has enough capacity and is initialized only after a complete copy
    let copied = unsafe {
        ReadProcessMemory(
            GetCurrentProcess(),
            address,
            image_path.as_mut_ptr().cast(),
            byte_len,
            &raw mut bytes_read,
        )
    };
    if copied == 0 || bytes_read != byte_len {
        return None;
    }
    // SAFETY: ReadProcessMemory initialized exactly image_path_len u16 values
    unsafe { image_path.set_len(image_path_len) };
    if image_path.contains(&0) {
        return None;
    }
    Some(image_path)
}

fn send_process_image(image_path: &[u16]) {
    // SAFETY: accessing the global client which was initialized during DLL_PROCESS_ATTACH
    unsafe { global_client() }
        .send(PathAccess { mode: AccessMode::READ, path: NativePath::from_wide(image_path) });
}

unsafe fn read_current_process_value<T>(address: *const c_void) -> Option<T> {
    let mut value = MaybeUninit::<T>::uninit();
    let mut bytes_read = 0;
    // SAFETY: ReadProcessMemory reports invalid source memory without dereferencing it in Rust
    let copied = unsafe {
        ReadProcessMemory(
            GetCurrentProcess(),
            address,
            value.as_mut_ptr().cast(),
            size_of::<T>(),
            &raw mut bytes_read,
        )
    };
    if copied == 0 || bytes_read != size_of::<T>() {
        return None;
    }
    // SAFETY: ReadProcessMemory initialized all bytes of value
    Some(unsafe { value.assume_init() })
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
