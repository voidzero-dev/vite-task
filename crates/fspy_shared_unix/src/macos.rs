//! macOS payload serialization and exec preparation shared between `fspy` and
//! `fspy_preload_macos`.

use allocator_api2::{alloc::Allocator, vec::Vec};
use bstr::BStr;
use fspy_nostd::{CWD, Error, Result};
use fspy_nostd_alloc::CString;
use fspy_shared::ipc::{IpcStr, channel::ChannelConf};
use phf::{Set, phf_set};
use wincode::{SchemaRead, SchemaWrite};

use crate::{
    codec::{decode_b64, encode_b64},
    nostd_exec::{
        Exec, append_path_env, ensure_env,
        open::{absolute_relative, open_executable, pread_header},
        shebang::{
            MACOS_MAX_LINE_LEN, MACOS_MAX_RECURSION, ParseShebangOptions, apply_shebang,
            parse_shebang_bytes,
        },
        to_vec_in,
    },
};

const PAYLOAD_ENV_NAME: &str = "FSPY_PAYLOAD";
const DYLD_INSERT_LIBRARIES: &[u8] = b"DYLD_INSERT_LIBRARIES";

/// Configuration transferred from `fspy` to `fspy_preload_macos` via the
/// process environment.
#[derive(Debug, SchemaWrite, SchemaRead)]
pub struct Payload<'a> {
    pub ipc_channel_conf: ChannelConf<'a>,
    pub preload_path: &'a IpcStr,
    pub bash_path: &'a IpcStr,
    pub coreutils_path: &'a IpcStr,
}

/// An opaque encoded macOS payload ready to inspect or forward to children.
pub struct EncodedPayload<'a> {
    payload: Payload<'a>,
    encoded_string: &'a str,
}

// Seals the view-only design: every field borrows leaked bytes.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<EncodedPayload<'static>>();
};

impl<'a> EncodedPayload<'a> {
    /// Encodes `payload` into its unpadded base64 form, leaking the encoded
    /// string into `allocator`.
    ///
    /// # Panics
    ///
    /// Panics if serialization or base64 encoding fails.
    #[must_use]
    pub fn encode<A: Allocator + Clone + 'a>(payload: Payload<'a>, allocator: A) -> Self {
        let encoded_string = encode_b64(&payload, allocator);
        Self { payload, encoded_string }
    }

    /// Decodes the macOS payload from an iterator over environment entries,
    /// leaking its backing bytes into `allocator`.
    ///
    /// Copies the encoded value out of the environment before decoding so it
    /// never borrows mutable `environ` storage.
    ///
    /// # Panics
    ///
    /// Panics when `FSPY_PAYLOAD` is missing, malformed base64, or fails
    /// deserialization.
    #[must_use]
    pub fn from_env<A: Allocator + Clone + 'a>(
        mut envs: impl Iterator<Item = fspy_nostd::env::Entry>,
        allocator: A,
    ) -> Self {
        let encoded_string: &'a str = envs
            .find_map(|(name, value)| {
                if AsRef::<[u8]>::as_ref(name) == PAYLOAD_ENV_NAME.as_bytes() {
                    value.map(|value| {
                        let copied = to_vec_in(value.as_units(), allocator.clone());
                        let leaked: &'a [u8] = copied.leak();
                        core::str::from_utf8(leaked).expect("FSPY_PAYLOAD is not valid UTF-8")
                    })
                } else {
                    None
                }
            })
            .expect("environment variable 'FSPY_PAYLOAD' not found");

        let payload = decode_b64(encoded_string.as_bytes(), allocator)
            .expect("invalid 'FSPY_PAYLOAD' payload");
        Self { payload, encoded_string }
    }

    /// Returns the shared-memory IPC channel configuration.
    #[must_use]
    pub const fn ipc_channel_conf(&self) -> ChannelConf<'a> {
        self.payload.ipc_channel_conf
    }

    /// Resolves shebangs, reports every read path, swaps SIP-protected
    /// `/bin`/`/usr/bin` utilities for bundled equivalents, and configures
    /// `DYLD_INSERT_LIBRARIES` + `FSPY_PAYLOAD` on `exec`.
    ///
    /// # Errors
    ///
    /// Returns OS-compatible `errno` values on empty program (`ENOENT`),
    /// inaccessible or non-regular executables (`ENOENT`/`EACCES`), malformed
    /// or nested shebangs (`ENOEXEC`), or conflicting `FSPY_PAYLOAD` values
    /// (`EINVAL`).
    pub fn prepare_exec<A: Allocator + Clone>(
        &self,
        exec: &mut Exec<A>,
        mut on_path_read: impl FnMut(&BStr),
    ) -> Result<()> {
        if exec.program.is_empty() {
            return Err(Error::NOENT);
        }

        let allocator = exec.allocator().clone();
        let mut peek_buf = [0u8; MACOS_MAX_LINE_LEN];
        let mut depth = 0usize;

        loop {
            if exec.program.is_empty() {
                return Err(Error::NOENT);
            }
            if exec.program.starts_with(b"/") {
                on_path_read(BStr::new(&exec.program));
            } else {
                let abs = absolute_relative(&exec.program, allocator.clone())?;
                on_path_read(BStr::new(&abs));
            }

            let c_path = bytes_to_cstring(&exec.program, allocator.clone())?;
            let file = open_executable(CWD, c_path.as_c_str().as_thin(), false)?;
            let n = pread_header(file.as_fd(), &mut peek_buf)?;
            drop(file);

            if let Some(shebang) = parse_shebang_bytes(
                &peek_buf[..n],
                ParseShebangOptions::macos(),
                allocator.clone(),
            )? {
                depth += 1;
                if depth > MACOS_MAX_RECURSION {
                    return Err(Error::NOEXEC);
                }
                let script_arg = to_vec_in(&exec.program, allocator.clone());
                apply_shebang(exec, shebang, script_arg);
                continue;
            }
            break;
        }

        if !exec.program.starts_with(b"/") {
            exec.program = absolute_relative(&exec.program, allocator.clone())?;
        }

        let injectable = match split_parent_file_name(&exec.program) {
            Some((b"/bin" | b"/usr/bin", file_name)) => {
                if matches!(file_name, b"sh" | b"bash") {
                    exec.program = to_vec_in(self.payload.bash_path.as_bytes(), allocator);
                    true
                } else if COREUTILS_FUNCTIONS_FOR_TEST.contains(file_name) {
                    exec.program = to_vec_in(self.payload.coreutils_path.as_bytes(), allocator);
                    true
                } else {
                    false
                }
            }
            _ => true,
        };

        if injectable {
            append_path_env(
                &mut exec.envs,
                DYLD_INSERT_LIBRARIES,
                self.payload.preload_path.as_bytes(),
            );
            ensure_env(
                &mut exec.envs,
                PAYLOAD_ENV_NAME.as_bytes(),
                self.encoded_string.as_bytes(),
            )?;
        } else {
            exec.envs.retain(|(name, _)| {
                name.as_slice() != DYLD_INSERT_LIBRARIES
                    && name.as_slice() != PAYLOAD_ENV_NAME.as_bytes()
            });
        }

        Ok(())
    }
}

fn bytes_to_cstring<A: Allocator>(
    bytes: &[u8],
    allocator: A,
) -> Result<CString<fspy_nostd::Fat, A>> {
    let mut owned = Vec::new_in(allocator);
    owned.try_reserve_exact(bytes.len() + 1).map_err(|_| Error::NOMEM)?;
    owned.extend_from_slice(bytes);
    owned.push(0);
    CString::from_vec_with_nul(owned).ok_or(Error::NOENT)
}

/// Splits an absolute path into `(parent, file_name)`, normalizing repeated
/// trailing or interior slashes before the final component.
fn split_parent_file_name(path: &[u8]) -> Option<(&[u8], &[u8])> {
    let trimmed = path.strip_suffix(b"/").unwrap_or(path);
    let slash = trimmed.iter().rposition(|&b| b == b'/')?;
    let file_name = &trimmed[slash + 1..];
    if file_name.is_empty() {
        return None;
    }
    let mut parent = &trimmed[..slash];
    while parent.len() > 1 && parent.ends_with(b"/") {
        parent = &parent[..parent.len() - 1];
    }
    if parent.is_empty() {
        parent = b"/";
    }
    Some((parent, file_name))
}

#[doc(hidden)]
pub static COREUTILS_FUNCTIONS_FOR_TEST: Set<&'static [u8]> = phf_set! {
    b"[", b"arch", b"b2sum", b"base32", b"base64", b"basename", b"basenc", b"cat",
    b"chgrp", b"chmod", b"chown", b"chroot", b"cksum", b"comm", b"cp", b"csplit",
    b"cut", b"date", b"dd", b"df", b"dir", b"dircolors", b"dirname", b"du", b"echo",
    b"env", b"expand", b"expr", b"factor", b"false", b"fmt", b"fold", b"groups",
    b"hashsum", b"head", b"hostid", b"hostname", b"id", b"install", b"join", b"kill",
    b"link", b"ln", b"logname", b"ls", b"md5sum", b"mkdir", b"mkfifo", b"mknod",
    b"mktemp", b"more", b"mv", b"nice", b"nl", b"nohup", b"nproc", b"numfmt", b"od",
    b"paste", b"pathchk", b"pinky", b"pr", b"printenv", b"printf", b"ptx", b"pwd",
    b"readlink", b"realpath", b"rm", b"rmdir", b"seq", b"sha1sum", b"sha224sum",
    b"sha256sum", b"sha384sum", b"sha512sum", b"shred", b"shuf", b"sleep", b"sort",
    b"split", b"stat", b"stdbuf", b"stty", b"sum", b"sync", b"tac", b"tail", b"tee",
    b"test", b"timeout", b"touch", b"tr", b"true", b"truncate", b"tsort", b"tty",
    b"uname", b"unexpand", b"uniq", b"unlink", b"uptime", b"users", b"vdir", b"wc",
    b"who", b"whoami", b"yes"
};

#[cfg(test)]
mod tests {
    use super::split_parent_file_name;

    #[test]
    fn splits_parent_and_file_name() {
        assert_eq!(
            split_parent_file_name(b"/bin/sh"),
            Some((b"/bin".as_slice(), b"sh".as_slice()))
        );
        assert_eq!(
            split_parent_file_name(b"/usr/bin/cat"),
            Some((b"/usr/bin".as_slice(), b"cat".as_slice()))
        );
        assert_eq!(
            split_parent_file_name(b"/bin//sh"),
            Some((b"/bin".as_slice(), b"sh".as_slice()))
        );
        assert_eq!(split_parent_file_name(b"/foo"), Some((b"/".as_slice(), b"foo".as_slice())));
        assert_eq!(split_parent_file_name(b"/"), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn nested_shebang_returns_enoexec_matching_xnu() {
        use std::{
            fs::{self, Permissions},
            os::unix::{ffi::OsStrExt as _, fs::PermissionsExt as _},
        };

        use allocator_api2::alloc::Global;
        use fspy_shared::ipc::{IpcStr, channel::channel};

        use super::{EncodedPayload, Payload};
        use crate::nostd_exec::{Exec, to_vec_in};

        let dir = tempfile::tempdir().unwrap();
        let inner = dir.path().join("inner.sh");
        let outer = dir.path().join("outer.sh");
        fs::write(&inner, b"#!/bin/sh\necho inner\n").unwrap();
        fs::set_permissions(&inner, Permissions::from_mode(0o755)).unwrap();
        fs::write(&outer, format!("#!{}\n", inner.display())).unwrap();
        fs::set_permissions(&outer, Permissions::from_mode(0o755)).unwrap();

        let receiver = channel(1 << 30, Global).unwrap();
        let payload = EncodedPayload::encode(
            Payload {
                ipc_channel_conf: receiver.conf(),
                preload_path: IpcStr::from_bytes(b"/tmp/preload.dylib"),
                bash_path: IpcStr::from_bytes(b"/tmp/bash"),
                coreutils_path: IpcStr::from_bytes(b"/tmp/coreutils"),
            },
            Global,
        );
        let mut exec = Exec {
            program: to_vec_in(outer.as_os_str().as_bytes(), Global),
            args: allocator_api2::vec::Vec::new_in(Global),
            envs: allocator_api2::vec::Vec::new_in(Global),
        };
        assert_eq!(payload.prepare_exec(&mut exec, |_| {}).unwrap_err(), fspy_nostd::Error::NOEXEC);
    }
}
