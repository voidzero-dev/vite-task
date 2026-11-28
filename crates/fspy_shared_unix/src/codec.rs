use allocator_api2::{alloc::Allocator, vec::Vec};
use base64::{Engine as _, prelude::BASE64_STANDARD_NO_PAD};
use wincode::{SchemaRead, SchemaWrite, config::DefaultConfig};

/// Serializes `value` with wincode and base64-encodes it without padding,
/// leaking the encoded UTF-8 string into `allocator`.
///
/// The intermediate binary buffer is dropped before returning.
#[must_use]
pub fn encode_b64<'a, T, A>(value: &T, allocator: A) -> &'a str
where
    T: SchemaWrite<DefaultConfig, Src = T>,
    A: Allocator + Clone + 'a,
{
    let serialized_size = usize::try_from(
        wincode::serialized_size(value).expect("payload cannot report its serialized size"),
    )
    .expect("serialized payload size exceeds usize");
    let mut buffer = Vec::with_capacity_in(serialized_size, allocator.clone());
    buffer.resize(serialized_size, 0);
    let mut writer: &mut [u8] = &mut buffer;
    wincode::serialize_into(&mut writer, value).expect("payload serialization failed");
    assert!(writer.is_empty(), "payload wrote fewer bytes than the size it reported");

    let encoded_len =
        base64::encoded_len(serialized_size, false).expect("encoded payload length exceeds usize");
    let mut encoded = Vec::with_capacity_in(encoded_len, allocator);
    encoded.resize(encoded_len, 0);
    let written =
        BASE64_STANDARD_NO_PAD.encode_slice(&buffer, &mut encoded).expect("base64 encoding failed");
    encoded.truncate(written);
    drop(buffer);

    let leaked: &'a [u8] = encoded.leak();
    core::str::from_utf8(leaked).expect("base64 output is valid UTF-8")
}

/// Base64-decodes `encoded` into `allocator` and deserializes `T` borrowing
/// from the leaked decoded bytes.
pub fn decode_b64<'a, T, A>(encoded: &[u8], allocator: A) -> Option<T>
where
    T: SchemaRead<'a, DefaultConfig, Dst = T>,
    A: Allocator + 'a,
{
    let decoded_len_estimate = base64::decoded_len_estimate(encoded.len());
    let mut buffer = Vec::with_capacity_in(decoded_len_estimate, allocator);
    buffer.resize(decoded_len_estimate, 0);
    let decoded_len = BASE64_STANDARD_NO_PAD.decode_slice(encoded, &mut buffer).ok()?;
    buffer.truncate(decoded_len);
    let leaked: &'a [u8] = buffer.leak();
    wincode::deserialize_exact(leaked).ok()
}
