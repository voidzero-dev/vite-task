//! Output archive creation and extraction using tar + zstd compression.

use std::fs::File;

use vite_path::{AbsolutePath, RelativePathBuf};

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
        // Skip files that no longer exist (task may delete temp files)
        if !abs_path.as_path().exists() {
            continue;
        }
        let metadata = std::fs::metadata(abs_path.as_path())?;
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
