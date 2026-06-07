#[derive(Debug)]
pub enum ShmemError {
    MapSizeZero,
    NoLinkOrOsId,
    FlinkInvalidOsId,
    LinkCreateFailed(std::io::Error),
    LinkWriteFailed(std::io::Error),
    LinkExists,
    LinkOpenFailed(std::io::Error),
    LinkReadFailed(std::io::Error),
    LinkDoesNotExist,
    MappingIdExists,
    MapCreateFailed(u32),
    MapOpenFailed(u32),
    UnknownOsError(u32),
}

impl std::fmt::Display for ShmemError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShmemError::MapSizeZero => f.write_str("You cannot create a shared memory mapping of 0 size"),
            ShmemError::NoLinkOrOsId => f.write_str("Tried to open mapping without flink path or os_id"),
            ShmemError::FlinkInvalidOsId => f.write_str("Tried to open mapping from both flink and os_id but the flink did not point to the same os_id"),
            ShmemError::LinkCreateFailed(err) => write!(f, "Creating the link file failed, {}", err),
            ShmemError::LinkWriteFailed(err) => write!(f, "Writing the link file failed, {}", err),
            ShmemError::LinkExists => f.write_str("Shared memory link already exists"),
            ShmemError::LinkOpenFailed(err) => write!(f, "Opening the link file failed, {}", err),
            ShmemError::LinkReadFailed(err) => write!(f, "Reading the link file failed, {}", err),
            ShmemError::LinkDoesNotExist => f.write_str("Requested link file does not exist"),
            ShmemError::MappingIdExists => f.write_str("Shared memory OS specific ID already exists"),
            ShmemError::MapCreateFailed(err) => write!(f, "Creating the shared memory failed, os error {}", err),
            ShmemError::MapOpenFailed(err) => write!(f, "Opening the shared memory failed, os error {}", err),
            ShmemError::UnknownOsError(err) => write!(f, "An unexpected OS error occurred, os error {}", err),
        }
    }
}

impl std::error::Error for ShmemError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ShmemError::LinkCreateFailed(err) => Some(err),
            ShmemError::LinkWriteFailed(err) => Some(err),
            ShmemError::LinkOpenFailed(err) => Some(err),
            ShmemError::LinkReadFailed(err) => Some(err),
            _ => None,
        }
    }
}
