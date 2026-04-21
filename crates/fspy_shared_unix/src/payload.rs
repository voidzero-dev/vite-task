use std::os::unix::ffi::OsStringExt;

use base64::{Engine as _, prelude::BASE64_STANDARD_NO_PAD};
use bstr::BString;
#[cfg(all(not(target_os = "android"), not(target_env = "musl")))]
use fspy_shared::ipc::NativeStr;
#[cfg(all(not(target_os = "android"), not(target_env = "musl")))]
use fspy_shared::ipc::channel::ChannelConf;
use wincode::{SchemaRead, SchemaWrite};

#[derive(Debug, SchemaWrite, SchemaRead)]
pub struct Payload {
    #[cfg(all(not(target_os = "android"), not(target_env = "musl")))]
    pub ipc_channel_conf: ChannelConf,

    #[cfg(all(not(target_os = "android"), not(target_env = "musl")))]
    pub preload_path: Box<NativeStr>,

    #[cfg(target_os = "macos")]
    pub artifacts: Artifacts,

    #[cfg(all(not(target_os = "android"), target_os = "linux"))]
    #[cfg_attr(
        not(target_env = "musl"),
        expect(clippy::struct_field_names, reason = "descriptive field name for clarity")
    )]
    pub seccomp_payload: fspy_seccomp_unotify::payload::SeccompPayload,
}

#[cfg(target_os = "macos")]
#[derive(Debug, SchemaWrite, SchemaRead, Clone)]
pub struct Artifacts {
    pub bash_path: Box<NativeStr>,
    pub coreutils_path: Box<NativeStr>,
}

pub(crate) const PAYLOAD_ENV_NAME: &str = "FSPY_PAYLOAD";

pub struct EncodedPayload {
    pub payload: Payload,
    pub encoded_string: BString,
}

/// Encodes the fspy payload into a base64 string for transmission via environment variable
///
/// # Panics
///
/// Panics if serialization fails, which should never happen for valid `Payload` structs.
#[must_use]
pub fn encode_payload(payload: Payload) -> EncodedPayload {
    let bytes = wincode::serialize(&payload).unwrap();
    let encoded_string = BASE64_STANDARD_NO_PAD.encode(&bytes);
    EncodedPayload { payload, encoded_string: encoded_string.into() }
}

/// Decodes the fspy payload from the environment variable
///
/// # Errors
///
/// Returns an error if:
/// - The environment variable is not found
/// - The base64 decoding fails
/// - The deserialization fails
pub fn decode_payload_from_env() -> anyhow::Result<EncodedPayload> {
    let Some(encoded_string) = std::env::var_os(PAYLOAD_ENV_NAME) else {
        anyhow::bail!("Environment variable '{PAYLOAD_ENV_NAME}' not found");
    };
    decode_payload(encoded_string.into_vec().into())
}

fn decode_payload(encoded_string: BString) -> anyhow::Result<EncodedPayload> {
    let bytes = BASE64_STANDARD_NO_PAD.decode(&encoded_string)?;
    let payload: Payload = wincode::deserialize_exact(&bytes)?;
    Ok(EncodedPayload { payload, encoded_string })
}
