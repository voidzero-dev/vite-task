// Stub fspy backend for OpenHarmony — fspy is not supported on OHOS.
// All types exist but operations will no-op or return empty results.

use std::{
    io,
    path::Path,
    process::ExitStatus,
};

use tokio_util::sync::CancellationToken;

use crate::{TrackedChild, command::Command, error::SpawnError};
pub use access_iter::PathAccessIterable;

mod access_iter {
    use fspy_shared::ipc::PathAccess;
    use std::path::Path;

    /// Empty access iterable for OHOS (no fspy tracking available).
    pub struct PathAccessIterable;

    impl PathAccessIterable {
        pub fn iter(&self) -> impl Iterator<Item = PathAccess<'_>> {
            Vec::new().into_iter()
        }
    }

    impl<'a> IntoIterator for &'a PathAccessIterable {
        type IntoIter = std::vec::IntoIter<Result<(&'a Path, PathAccess<'a>), (&'a Path, std::io::Error)>>;
        type Item = Result<(&'a Path, PathAccess<'a>), (&'a Path, std::io::Error)>;

        fn into_iter(self) -> Self::IntoIter {
            Vec::new().into_iter()
        }
    }
}

#[derive(Debug)]
pub struct SpyImpl;

impl SpyImpl {
    pub fn init_in(_dir: &Path) -> io::Result<Self> {
        Ok(SpyImpl)
    }

    pub fn new(_resolved_program: &Path) -> io::Result<Self> {
        Ok(SpyImpl)
    }

    pub(crate) async fn spawn(
        &self,
        command: Command,
        _cancellation_token: CancellationToken,
    ) -> Result<TrackedChild, SpawnError> {
        let mut tokio_cmd = command.into_tokio_command();
        let mut child = tokio_cmd.spawn().map_err(|e| SpawnError::OsSpawn(e))?;

        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let wait_handle = async move {
            let status = child.wait().await.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
            Ok(crate::ChildTermination {
                status,
                path_accesses: PathAccessIterable,
            })
        };

        Ok(TrackedChild {
            stdin,
            stdout,
            stderr,
            wait_handle: Box::pin(wait_handle),
            #[cfg(windows)]
            process_handle: std::os::windows::io::OwnedHandle::from(std::fs::File::open("/dev/null")?),
        })
    }
}
