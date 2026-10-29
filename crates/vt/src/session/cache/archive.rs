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
/// On Windows, a symbolic link is an error: `tar` restores every link there
/// with `symlink_file`, which can't link a directory, and Windows doesn't
/// resolve a relative link target with the `/` separators `tar` stores.
///
/// # Errors
///
/// Returns an error if creating the archive file or adding entries fails, or
/// on Windows if an output is a symbolic link.
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
            if cfg!(windows) {
                anyhow::bail!(
                    "output `{rel_path}` is a symbolic link, which can't be cached on Windows"
                );
            }
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
/// On Windows, a symbolic link in the archive is an error, for the reasons
/// given on [`create_output_archive`].
///
/// # Errors
///
/// Returns an error if opening the archive or extracting entries fails, or on
/// Windows if the archive holds a symbolic link.
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
    unpack_without_symlinks(&mut archive, workspace_root)?;
    Ok(())
}

/// [`tar::Archive::unpack`], except that a symbolic link is an error.
#[cfg(windows)]
fn unpack_without_symlinks(
    archive: &mut tar::Archive<impl Read>,
    workspace_root: &AbsolutePath,
) -> anyhow::Result<()> {
    // As `tar::Archive::unpack` does: the canonical root is a verbatim (`\\?\`)
    // path, which lifts Windows' path length limit.
    let root = std::fs::canonicalize(workspace_root.as_path())
        .unwrap_or_else(|_| workspace_root.as_path().to_path_buf());
    for entry in archive.entries()? {
        let mut entry = entry?;
        if entry.header().entry_type().is_symlink() {
            let path = entry.path()?;
            let path = path.display();
            anyhow::bail!(
                "the archive holds the symbolic link `{path}`, which can't be restored on Windows"
            );
        }
        entry.unpack_in(&root)?;
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

    #[cfg(unix)]
    #[test]
    fn symlinks_round_trip_as_links() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = AbsolutePathBuf::new(tmp.path().to_path_buf()).unwrap();
        fs::create_dir_all(dir.join("dist/pkg")).unwrap();
        fs::write(dir.join("dist/pkg/index.js"), "export default 42;").unwrap();
        let links = [("file.js", "pkg/index.js"), ("dependency", "pkg"), ("dangling", "missing")];
        for (name, target) in links {
            std::os::unix::fs::symlink(target, dir.join("dist").join(name)).unwrap();
        }
        let files = ["dist/pkg/index.js", "dist/file.js", "dist/dependency", "dist/dangling"]
            .map(|path| RelativePathBuf::new(path).unwrap());
        let archive_path = dir.join("output.tar.zst");
        create_output_archive(&dir, &files, &archive_path).unwrap();

        fs::remove_dir_all(dir.join("dist")).unwrap();
        extract_output_archive(&dir, &archive_path).unwrap();
        for (name, target) in links {
            let restored = fs::read_link(dir.join("dist").join(name)).unwrap();
            assert_eq!(restored.to_str(), Some(target));
        }
        assert_eq!(fs::read_to_string(dir.join("dist/file.js")).unwrap(), "export default 42;");
        assert_eq!(
            fs::read_to_string(dir.join("dist/dependency/index.js")).unwrap(),
            "export default 42;"
        );
        assert!(!dir.join("dist/dangling").as_path().exists());
    }

    #[cfg(unix)]
    #[test]
    fn restore_rejects_writing_through_a_link_outside_the_workspace() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = AbsolutePathBuf::new(tmp.path().to_path_buf()).unwrap();
        let outside = tempfile::TempDir::new().unwrap();
        let sentinel = outside.path().join("sentinel.txt");
        fs::write(&sentinel, "archived").unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.join("link")).unwrap();
        let files = ["link", "link/sentinel.txt"].map(|path| RelativePathBuf::new(path).unwrap());
        let archive_path = dir.join("output.tar.zst");
        create_output_archive(&dir, &files, &archive_path).unwrap();
        fs::write(&sentinel, "must remain unchanged").unwrap();

        let restored = dir.join("restored");
        fs::create_dir_all(&restored).unwrap();
        assert!(extract_output_archive(&restored, &archive_path).is_err());
        assert_eq!(fs::read_to_string(&sentinel).unwrap(), "must remain unchanged");
    }

    #[cfg(windows)]
    #[test]
    fn create_rejects_symlinks_on_windows() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = AbsolutePathBuf::new(tmp.path().to_path_buf()).unwrap();
        fs::write(dir.join("index.js"), "export default 42;").unwrap();
        std::os::windows::fs::symlink_file("index.js", dir.join("link.js")).unwrap();
        let files = ["index.js", "link.js"].map(|path| RelativePathBuf::new(path).unwrap());

        let err = create_output_archive(&dir, &files, &dir.join("output.tar.zst")).unwrap_err();
        assert_eq!(
            err.to_string(),
            "output `link.js` is a symbolic link, which can't be cached on Windows"
        );
    }

    #[cfg(windows)]
    #[test]
    fn extract_rejects_symlinks_on_windows() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = AbsolutePathBuf::new(tmp.path().to_path_buf()).unwrap();
        let archive_path = dir.join("output.tar.zst");
        {
            let file = File::create(archive_path.as_path()).unwrap();
            let mut builder = tar::Builder::new(zstd::Encoder::new(file, 0).unwrap().auto_finish());
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_size(0);
            builder.append_link(&mut header, "link.js", "index.js").unwrap();
            builder.finish().unwrap();
        }

        let err = extract_output_archive(&dir, &archive_path).unwrap_err();
        assert_eq!(
            err.to_string(),
            "the archive holds the symbolic link `link.js`, which can't be restored on Windows"
        );
        assert!(fs::symlink_metadata(dir.join("link.js")).is_err());
    }
}
