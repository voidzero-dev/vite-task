use std::{ffi::OsStr, io, io::Write as _};

use pipe_socket::Client;
use wincode::Serialize as _;

use super::PathAccess;

const FRAME_HEADER_LEN: usize = size_of::<u32>();

/// A synchronous, single-threaded sender for framed path-access records.
pub struct PathAccessSender {
    client: Client,
    frame: Vec<u8>,
}

impl PathAccessSender {
    /// Connects to an fspy supervisor.
    ///
    /// # Errors
    ///
    /// Returns an error if the pipe socket connection cannot be established.
    pub fn connect(server_name: &OsStr) -> io::Result<Self> {
        Ok(Self { client: Client::connect(server_name)?, frame: Vec::new() })
    }

    /// Serializes and sends one path-access record.
    ///
    /// # Errors
    ///
    /// Returns an error if the framed record cannot be written to the pipe.
    ///
    /// # Panics
    ///
    /// Panics if the serialized record is larger than `u32::MAX` bytes or if
    /// serialization produces an inconsistent size.
    pub fn send(&mut self, access: PathAccess<'_>) -> io::Result<()> {
        let payload_len = usize::try_from(
            PathAccess::serialized_size(&access).expect("failed to size PathAccess"),
        )
        .expect("serialized PathAccess size exceeds usize");
        let payload_len_u32 =
            u32::try_from(payload_len).expect("serialized PathAccess size exceeds u32");

        self.frame.clear();
        self.frame.extend_from_slice(&payload_len_u32.to_le_bytes());
        self.frame.resize(FRAME_HEADER_LEN + payload_len, 0);

        let mut payload = &mut self.frame[FRAME_HEADER_LEN..];
        PathAccess::serialize_into(&mut payload, &access).expect("failed to serialize PathAccess");
        debug_assert!(payload.is_empty());

        self.client.write_all(&self.frame)
    }
}
