//! Output archive creation and extraction using tar + zstd compression.

use std::{
    fs::File,
    io::{self, Read},
};

use vt_path::{AbsolutePath, RelativePathBuf};

/// Create a tar.zst archive from workspace-relative output paths.
///
/// A regular file is stored with its contents. A symbolic link is stored as a
/// link with its target, never followed, so a link to a directory or a
/// dangling link round-trips as the task left it. Paths that no longer exist
/// are silently skipped (the task may delete temporary files during
/// execution).
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
        let metadata = match std::fs::symlink_metadata(abs_path.as_path()) {
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
        } else if metadata.file_type().is_symlink() {
            let target = std::fs::read_link(abs_path.as_path())?;
            let mut header = tar::Header::new_gnu();
            header.set_metadata(&metadata);
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_size(0);
            builder.append_link(&mut header, rel_path.as_str(), target)?;
        }
    }

    builder.finish()?;
    Ok(())
}

/// Extract a tar.zst archive, restoring files and symbolic links relative to
/// workspace root.
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

    #[cfg(not(windows))]
    archive.unpack(workspace_root.as_path())?;
    #[cfg(windows)]
    unpack_with_directory_links(&mut archive, workspace_root)?;
    Ok(())
}

/// [`tar::Archive::unpack`], except that a restored link whose target is a
/// directory becomes a directory link: `tar` creates every link with
/// `symlink_file` on Windows, and Windows doesn't follow a file link to a
/// directory. Links are fixed up once every entry is unpacked, so their targets
/// exist when they are checked.
#[cfg(windows)]
fn unpack_with_directory_links(
    archive: &mut tar::Archive<impl Read>,
    workspace_root: &AbsolutePath,
) -> anyhow::Result<()> {
    // As `tar::Archive::unpack` does: the canonical root is a verbatim (`\\?\`)
    // path, which lifts Windows' path length limit.
    let root = std::fs::canonicalize(workspace_root.as_path())
        .unwrap_or_else(|_| workspace_root.as_path().to_path_buf());
    let mut links = Vec::new();
    for entry in archive.entries()? {
        let mut entry = entry?;
        let is_link = entry.header().entry_type().is_symlink();
        let link = root.join(entry.path()?);
        if entry.unpack_in(&root)? && is_link {
            links.push(link);
        }
    }
    for link in links {
        let target = std::fs::read_link(&link)?;
        if link.parent().is_some_and(|parent| parent.join(&target).is_dir()) {
            std::fs::remove_file(&link)?;
            std::os::windows::fs::symlink_dir(&target, &link)?;
        }
    }
    Ok(())
}

/// Read a tar.zst archive from `reader` to the end without writing any files,
/// to check that it decodes.
///
/// # Errors
///
/// Returns an error if reading fails or the archive doesn't decode.
pub fn check_output_archive(reader: impl Read) -> io::Result<()> {
    let mut archive = tar::Archive::new(zstd::Decoder::new(reader)?);
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
    use std::fs;

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
        let archive = std::fs::read(archive_path.as_path()).unwrap();
        check_output_archive(archive.as_slice()).unwrap();

        for corrupt in [&archive[..archive.len() - 1], b"corrupt"] {
            assert!(check_output_archive(corrupt).is_err());
        }
    }

    fn symlink(target: &str, link: &AbsolutePath, directory: bool) {
        #[cfg(unix)]
        {
            let _ = directory;
            std::os::unix::fs::symlink(target, link.as_path()).unwrap();
        }
        #[cfg(windows)]
        if directory {
            std::os::windows::fs::symlink_dir(target, link.as_path()).unwrap();
        } else {
            std::os::windows::fs::symlink_file(target, link.as_path()).unwrap();
        }
    }

    #[test]
    fn symlinks_round_trip_as_links() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = AbsolutePathBuf::new(tmp.path().to_path_buf()).unwrap();
        fs::create_dir_all(dir.join("dist/pkg")).unwrap();
        fs::write(dir.join("dist/pkg/index.js"), "export default 42;").unwrap();
        let links = [
            ("file.js", "pkg/index.js", false),
            ("dependency", "pkg", true),
            ("dangling", "missing", false),
        ];
        for (name, target, directory) in links {
            symlink(target, &dir.join("dist").join(name), directory);
        }
        let files = ["dist/pkg/index.js", "dist/file.js", "dist/dependency", "dist/dangling"]
            .map(|path| RelativePathBuf::new(path).unwrap());
        let archive_path = dir.join("output.tar.zst");
        create_output_archive(&dir, &files, &archive_path).unwrap();

        fs::remove_dir_all(dir.join("dist")).unwrap();
        extract_output_archive(&dir, &archive_path).unwrap();
        for (name, target, _) in links {
            let restored = fs::read_link(dir.join("dist").join(name)).unwrap();
            assert_eq!(
                RelativePathBuf::new(restored).unwrap(),
                RelativePathBuf::new(target).unwrap()
            );
        }
        assert_eq!(fs::read_to_string(dir.join("dist/file.js")).unwrap(), "export default 42;");
        assert_eq!(
            fs::read_to_string(dir.join("dist/dependency/index.js")).unwrap(),
            "export default 42;"
        );
        assert!(!dir.join("dist/dangling").as_path().exists());
    }

    #[test]
    fn restore_rejects_writing_through_a_link_outside_the_workspace() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = AbsolutePathBuf::new(tmp.path().to_path_buf()).unwrap();
        let outside = tempfile::TempDir::new().unwrap();
        let sentinel = outside.path().join("sentinel.txt");
        fs::write(&sentinel, "archived").unwrap();
        symlink(outside.path().to_str().unwrap(), &dir.join("link"), true);
        let files = ["link", "link/sentinel.txt"].map(|path| RelativePathBuf::new(path).unwrap());
        let archive_path = dir.join("output.tar.zst");
        create_output_archive(&dir, &files, &archive_path).unwrap();
        fs::write(&sentinel, "must remain unchanged").unwrap();

        let restored = dir.join("restored");
        fs::create_dir_all(&restored).unwrap();
        assert!(extract_output_archive(&restored, &archive_path).is_err());
        assert_eq!(fs::read_to_string(&sentinel).unwrap(), "must remain unchanged");
    }
}
