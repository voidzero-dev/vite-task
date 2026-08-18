use allocator_api2::alloc::Allocator;
use base64::{Engine as _, prelude::BASE64_STANDARD_NO_PAD};
use bstr::BStr;
#[cfg(not(target_env = "musl"))]
use fspy_shared::ipc::IpcStr;
#[cfg(not(target_env = "musl"))]
use fspy_shared::ipc::channel::ChannelConf;
use wincode::{SchemaRead, SchemaWrite};

/// The payload as it travels between processes.
///
/// A payload is a view: every path in it borrows from storage its producer
/// owns — the supervisor's session state, or the storage a
/// [`decode_payload_from_env`] caller supplies — so serializing one, and
/// deserializing one in the preload, allocates nothing for the borrowed
/// fields.
#[derive(Debug, SchemaWrite, SchemaRead)]
pub struct Payload<'a> {
    #[cfg(not(target_env = "musl"))]
    pub ipc_channel_conf: ChannelConf<'a>,
    #[cfg(target_env = "musl")]
    pub ipc_channel_conf: core::marker::PhantomData<&'a ()>,

    #[cfg(not(target_env = "musl"))]
    pub preload_path: &'a IpcStr,

    #[cfg(target_os = "macos")]
    pub artifacts: Artifacts<'a>,

    #[cfg(target_os = "linux")]
    #[cfg_attr(
        not(target_env = "musl"),
        expect(clippy::struct_field_names, reason = "descriptive field name for clarity")
    )]
    pub seccomp_payload: fspy_seccomp_unotify::payload::SeccompPayload,
}

#[cfg(target_os = "macos")]
#[derive(Debug, SchemaWrite, SchemaRead, Clone, Copy)]
pub struct Artifacts<'a> {
    pub bash_path: &'a IpcStr,
    pub coreutils_path: &'a IpcStr,
}

pub(crate) const PAYLOAD_ENV_NAME: &str = "FSPY_PAYLOAD";

/// A payload together with its encoded form, for handing to child processes.
///
/// Like [`Payload`], this is strictly a view: it lives as long as the
/// storage [`decode_payload_from_env`] allocated into.
pub struct EncodedPayload<'a> {
    pub payload: Payload<'a>,
    pub encoded_string: &'a BStr,
}

// Seals the view-only design: every field is an inert borrow of leaked
// bytes. A retained allocator handle is interior-mutable and would fail
// this assertion.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<EncodedPayload<'static>>();
};

/// Encodes the payload into its base64 environment form and assembles the
/// [`EncodedPayload`] that travels to child processes.
///
/// The serialization buffer is transient; the encoded string is leaked
/// into `allocator`, and `A: 'a` bounds the result's lifetime just as in
/// [`decode_payload_from_env`].
///
/// # Panics
///
/// Panics if serialization fails, which should never happen for valid
/// [`Payload`] structs.
#[must_use]
pub fn encode_payload<'a, A: Allocator + Clone + 'a>(
    payload: Payload<'a>,
    allocator: A,
) -> EncodedPayload<'a> {
    let serialized_size = usize::try_from(wincode::serialized_size(&payload).unwrap())
        .expect("serialized size exceeds usize");
    let mut buffer = allocator_api2::vec::Vec::with_capacity_in(serialized_size, allocator.clone());
    buffer.resize(serialized_size, 0);
    let mut writer: &mut [u8] = &mut buffer;
    wincode::serialize_into(&mut writer, &payload).unwrap();
    assert!(writer.is_empty(), "the payload wrote fewer bytes than the size it reported");

    let encoded_len =
        base64::encoded_len(serialized_size, false).expect("encoded payload length exceeds usize");
    let mut encoded = allocator_api2::vec::Vec::with_capacity_in(encoded_len, allocator);
    encoded.resize(encoded_len, 0);
    let written = BASE64_STANDARD_NO_PAD.encode_slice(&buffer, &mut encoded).unwrap();
    encoded.truncate(written);
    EncodedPayload { payload, encoded_string: BStr::new(encoded.leak()) }
}

/// Decodes the fspy payload from an iterator over environment entries.
///
/// The returned payload borrows allocations that are leaked into
/// `allocator` and never individually freed. `A: 'a` bounds the payload's
/// lifetime: a borrowed bump yields a payload that dies with the borrow,
/// and a `'static` allocator yields a `'static` payload outright.
///
/// The payload never borrows the process environment itself: the
/// environment is not stable storage (any `setenv` may move or rewrite it),
/// so the encoded value is copied out before it is used.
///
/// # Errors
///
/// Returns an error if the payload environment variable is missing, base64
/// decoding fails, or deserialization fails.
pub fn decode_payload_from_env<'a, A: Allocator + Clone + 'a>(
    mut envs: impl Iterator<Item = fspy_nostd::env::Entry>,
    allocator: A,
) -> anyhow::Result<EncodedPayload<'a>> {
    let Some(encoded_string) = envs.find_map(|(name, value)| {
        if AsRef::<[u8]>::as_ref(name) == PAYLOAD_ENV_NAME.as_bytes() {
            value.map(|value| {
                let mut encoded = allocator_api2::vec::Vec::with_capacity_in(
                    value.as_units().len(),
                    allocator.clone(),
                );
                encoded.extend_from_slice(value.as_units());
                BStr::new(encoded.leak())
            })
        } else {
            None
        }
    }) else {
        anyhow::bail!("Environment variable '{PAYLOAD_ENV_NAME}' not found");
    };

    let decoded_len_estimate = base64::decoded_len_estimate(encoded_string.len());
    let mut buffer = allocator_api2::vec::Vec::with_capacity_in(decoded_len_estimate, allocator);
    buffer.resize(decoded_len_estimate, 0);
    let decoded_len = BASE64_STANDARD_NO_PAD.decode_slice(encoded_string, &mut buffer)?;
    buffer.truncate(decoded_len);
    let payload: Payload<'a> = wincode::deserialize_exact(buffer.leak())?;

    Ok(EncodedPayload { payload, encoded_string })
}
