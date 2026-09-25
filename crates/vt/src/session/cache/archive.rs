//! Output archive creation and extraction using tar + zstd compression.

use std::{fs::File, io};

use vt_path::{AbsolutePath, RelativePathBuf};

/// Create a tar.zst archive from workspace-relative output file paths.
///
/// Files that no longer exist are silently skipped (the task may delete
/// temporary files during execution).
///
/// # Errors
///
/// Returns an error if creating the archive file or adding entries fails.
pub fn create_output_archive(
    workspace_root: &AbsolutePath,
    output_files: &[RelativePathBuf],
    archive_path: &AbsolutePath,
) -> anyhow::Result<()> {
    let file = File::create(archive_path.as_path())?;
    let encoder = zstd::Encoder::new(file, 0)?.auto_finish();
    let mut builder = tar::Builder::new(encoder);

    for rel_path in output_files {
        let abs_path = workspace_root.join(rel_path);
        // Skip files that no longer exist (task may delete temp files between
        // glob walk and archiving). Any other error is propagated.
        let metadata = match std::fs::metadata(abs_path.as_path()) {
            Ok(m) => m,
            Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err.into()),
        };
        if metadata.is_file() {
            let mut file = File::open(abs_path.as_path())?;
            let mut header = tar::Header::new_gnu();
            header.set_metadata(&metadata);
            header.set_cksum();
            builder.append_data(&mut header, rel_path.as_str(), &mut file)?;
        }
    }

    builder.finish()?;
    Ok(())
}

/// Extract a tar.zst archive, restoring files relative to workspace root.
///
/// Parent directories are created automatically. Existing files are overwritten.
///
/// # Errors
///
/// Returns an error if opening the archive or extracting entries fails.
pub fn extract_output_archive(
    workspace_root: &AbsolutePath,
    archive_path: &AbsolutePath,
) -> anyhow::Result<()> {
    let file = File::open(archive_path.as_path())?;
    let decoder = zstd::Decoder::new(file)?;
    let mut archive = tar::Archive::new(decoder);

    archive.unpack(workspace_root.as_path())?;
    Ok(())
}

/// Read a tar.zst archive to the end without writing any files, to check that
/// it decodes.
///
/// # Errors
///
/// Returns an error if opening the archive fails or it doesn't decode.
pub fn check_output_archive(archive_path: &AbsolutePath) -> io::Result<()> {
    let file = File::open(archive_path.as_path())?;
    let mut archive = tar::Archive::new(zstd::Decoder::new(file)?);
    for entry in archive.entries()? {
        io::copy(&mut entry?, &mut io::sink())?;
    }
    // Decode the rest of the stream too, so a truncated zstd frame after the
    // end of the tar data is caught.
    io::copy(&mut archive.into_inner(), &mut io::sink())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use vt_path::AbsolutePathBuf;

    use super::*;

    #[test]
    fn check_accepts_only_complete_archives() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = AbsolutePathBuf::new(tmp.path().to_path_buf()).unwrap();
        let output = RelativePathBuf::new("output.txt").unwrap();
        std::fs::write(dir.join(&output).as_path(), "built\n".repeat(1000)).unwrap();
        let archive_path = dir.join("output.tar.zst");
        create_output_archive(&dir, &[output], &archive_path).unwrap();
        check_output_archive(&archive_path).unwrap();

        let archive = std::fs::read(archive_path.as_path()).unwrap();
        for corrupt in [&archive[..archive.len() - 1], b"corrupt"] {
            std::fs::write(archive_path.as_path(), corrupt).unwrap();
            assert!(check_output_archive(&archive_path).is_err());
        }
    }
}
