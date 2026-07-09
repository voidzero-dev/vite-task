use fspy_seccomp_unotify::{payload::SeccompPayload, target::install_target};

#[cfg(not(target_env = "musl"))]
use crate::exec::append_path_env;
use crate::{
    exec::{Exec, ensure_env},
    payload::{EncodedPayload, PAYLOAD_ENV_NAME},
};

#[cfg(not(target_env = "musl"))]
const LD_PRELOAD: &str = "LD_PRELOAD";

pub struct PreExec(SeccompPayload);
impl PreExec {
    /// Installs the seccomp unotify filter for the current process.
    ///
    /// # Errors
    ///
    /// Returns an error if the seccomp filter installation fails.
    pub fn run(&self) -> nix::Result<()> {
        install_target(&self.0)
    }
}

pub fn handle_exec(
    command: &mut Exec,
    encoded_payload: &EncodedPayload,
    install_listener: bool,
) -> nix::Result<Option<PreExec>> {
    #[cfg(not(target_env = "musl"))]
    {
        use std::os::unix::ffi::OsStrExt as _;

        // Static targets ignore LD_PRELOAD but must carry it to dynamic descendants.
        append_path_env(
            &mut command.envs,
            LD_PRELOAD,
            encoded_payload.payload.preload_path.as_os_str().as_bytes(),
        );
    }
    ensure_env(&mut command.envs, PAYLOAD_ENV_NAME, &encoded_payload.encoded_string)?;

    Ok(install_listener.then(|| PreExec(encoded_payload.payload.seccomp_payload.clone())))
}
