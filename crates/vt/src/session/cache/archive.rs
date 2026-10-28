//! Output archive creation and extraction using tar + zstd compression.

use std::{
    fs::File,
    io::{self, Read},
    path::Component,
};

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
/// A file that already has its entry's content and permissions is left as it
/// is, modification time included, so restoring outputs that didn't change
/// writes nothing and file watchers see no change. Every other entry is
/// unpacked as [`tar::Archive::unpack`] would: parent directories are created
/// and an existing file is replaced. (`Archive::unpack` also defers directory
/// entries to the end, and [`create_output_archive`] writes none.)
///
/// # Errors
///
/// Returns an error if opening the archive or extracting entries fails.
pub fn extract_output_archive(
    workspace_root: &AbsolutePath,
    archive_path: &AbsolutePath,
) -> anyhow::Result<()> {
    // Canonicalized once, as `Archive::unpack` does: on Windows this gives the
    // `\\?\` prefix that lifts the 260-character path limit.
    let dst = workspace_root
        .as_path()
        .canonicalize()
        .unwrap_or_else(|_| workspace_root.as_path().to_path_buf());

    // Comparing an entry with a file of the same size reads the entry's data,
    // so an entry found to differ that way is unpacked in a second pass.
    let mut read_and_differ = Vec::new();
    let mut archive = open_output_archive(archive_path)?;
    for (index, entry) in archive.entries()?.enumerate() {
        let mut entry = entry?;
        match compare_with_disk(workspace_root, &mut entry)? {
            OnDisk::Matches => {}
            OnDisk::Differs => {
                entry.unpack_in(&dst)?;
            }
            OnDisk::DiffersAfterRead => read_and_differ.push(index),
        }
    }

    let mut pending = read_and_differ.into_iter().peekable();
    if pending.peek().is_some() {
        let mut archive = open_output_archive(archive_path)?;
        for (index, entry) in archive.entries()?.enumerate() {
            let Some(&next) = pending.peek() else { break };
            let mut entry = entry?;
            if index == next {
                entry.unpack_in(&dst)?;
                pending.next();
            }
        }
    }
    Ok(())
}

fn open_output_archive(
    archive_path: &AbsolutePath,
) -> io::Result<tar::Archive<zstd::Decoder<'static, io::BufReader<File>>>> {
    let file = File::open(archive_path.as_path())?;
    Ok(tar::Archive::new(zstd::Decoder::new(file)?))
}

/// How the file an archive entry restores compares with the entry.
enum OnDisk {
    /// The file has the entry's content and permissions.
    Matches,
    /// The entry isn't a regular file, or the file is missing or differs in
    /// type, size or permissions. The entry's data is unread.
    Differs,
    /// The file differs in content, found by reading the entry's data.
    DiffersAfterRead,
}

fn compare_with_disk(
    workspace_root: &AbsolutePath,
    entry: &mut tar::Entry<'_, impl Read>,
) -> io::Result<OnDisk> {
    if !entry.header().entry_type().is_file() {
        return Ok(OnDisk::Differs);
    }
    let Ok(mode) = entry.header().mode() else {
        return Ok(OnDisk::Differs);
    };
    let target = match entry.path() {
        // Any other path is left to `unpack_in`, which skips or rejects it.
        Ok(path) if path.components().all(|part| matches!(part, Component::Normal(_))) => {
            workspace_root.join(path)
        }
        _ => return Ok(OnDisk::Differs),
    };
    let Ok(metadata) = std::fs::symlink_metadata(target.as_path()) else {
        return Ok(OnDisk::Differs);
    };
    if !metadata.is_file()
        || metadata.len() != entry.size()
        || !has_unpacked_permissions(&metadata, mode)
    {
        return Ok(OnDisk::Differs);
    }
    let Ok(on_disk) = File::open(target.as_path()) else {
        return Ok(OnDisk::Differs);
    };
    Ok(if same_content(entry, on_disk)? { OnDisk::Matches } else { OnDisk::DiffersAfterRead })
}

/// Whether two readers of the same length yield the same bytes. Only errors
/// reading `archived` are returned; one reading `on_disk` counts as a
/// difference.
fn same_content(mut archived: impl Read, mut on_disk: impl Read) -> io::Result<bool> {
    let mut archived_buf = [0; 16 * 1024];
    let mut on_disk_buf = [0; 16 * 1024];
    loop {
        let len = archived.read(&mut archived_buf)?;
        if len == 0 {
            return Ok(true);
        }
        if on_disk.read_exact(&mut on_disk_buf[..len]).is_err()
            || archived_buf[..len] != on_disk_buf[..len]
        {
            return Ok(false);
        }
    }
}

/// Whether a file has the permissions `unpack_in` gives a file from an entry
/// with `mode`: its permission bits, without setuid, setgid or sticky.
#[cfg(unix)]
fn has_unpacked_permissions(metadata: &std::fs::Metadata, mode: u32) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o7777 == mode & 0o777
}

/// Whether a file has the permissions `unpack_in` gives a file from an entry
/// with `mode`: read-only when the mode lacks the owner's write bit.
#[cfg(windows)]
fn has_unpacked_permissions(metadata: &std::fs::Metadata, mode: u32) -> bool {
    metadata.permissions().readonly() == (mode & 0o200 == 0)
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

    /// Writes each `(name, content)` output into a temporary workspace and
    /// archives them all.
    fn archived_outputs(outputs: &[(&str, &str)]) -> (tempfile::TempDir, AbsolutePathBuf) {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = AbsolutePathBuf::new(tmp.path().to_path_buf()).unwrap();
        let mut paths = Vec::new();
        for (name, content) in outputs {
            let path = RelativePathBuf::new(name).unwrap();
            std::fs::write(dir.join(&path).as_path(), content).unwrap();
            paths.push(path);
        }
        let archive_path = dir.join("output.tar.zst");
        create_output_archive(&dir, &paths, &archive_path).unwrap();
        (tmp, archive_path)
    }

    /// A modification time long before the archive was made, which a restore
    /// that rewrites the file replaces with the archived one.
    fn mark_untouched(path: &AbsolutePath) -> std::time::SystemTime {
        let time = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
        File::options().write(true).open(path.as_path()).unwrap().set_modified(time).unwrap();
        time
    }

    fn modified(path: &AbsolutePath) -> std::time::SystemTime {
        std::fs::metadata(path.as_path()).unwrap().modified().unwrap()
    }

    #[test]
    fn extract_leaves_matching_files_untouched() {
        let (tmp, archive_path) = archived_outputs(&[("a.txt", "a\n"), ("b.txt", "b\n")]);
        let dir = AbsolutePathBuf::new(tmp.path().to_path_buf()).unwrap();
        let a = dir.join("a.txt");
        let b = dir.join("b.txt");
        let untouched = [mark_untouched(&a), mark_untouched(&b)];

        extract_output_archive(&dir, &archive_path).unwrap();

        assert_eq!([modified(&a), modified(&b)], untouched);
        assert_eq!(std::fs::read_to_string(a.as_path()).unwrap(), "a\n");
        assert_eq!(std::fs::read_to_string(b.as_path()).unwrap(), "b\n");
    }

    #[test]
    fn extract_restores_missing_and_changed_files() {
        let (tmp, archive_path) = archived_outputs(&[
            ("missing.txt", "missing\n"),
            ("resized.txt", "resized\n"),
            ("same-size.txt", "same size\n"),
            ("unchanged.txt", "unchanged\n"),
        ]);
        let dir = AbsolutePathBuf::new(tmp.path().to_path_buf()).unwrap();
        std::fs::remove_file(dir.join("missing.txt").as_path()).unwrap();
        std::fs::write(dir.join("resized.txt").as_path(), "longer content\n").unwrap();
        std::fs::write(dir.join("same-size.txt").as_path(), "SAME SIZE\n").unwrap();
        let unchanged = dir.join("unchanged.txt");
        let untouched = mark_untouched(&unchanged);

        extract_output_archive(&dir, &archive_path).unwrap();

        for (name, content) in [
            ("missing.txt", "missing\n"),
            ("resized.txt", "resized\n"),
            ("same-size.txt", "same size\n"),
            ("unchanged.txt", "unchanged\n"),
        ] {
            assert_eq!(std::fs::read_to_string(dir.join(name).as_path()).unwrap(), content);
        }
        assert_eq!(modified(&unchanged), untouched);
    }

    #[test]
    fn extract_restores_changed_permissions() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = AbsolutePathBuf::new(tmp.path().to_path_buf()).unwrap();
        let path = RelativePathBuf::new("output.txt").unwrap();
        let output = dir.join(&path);
        std::fs::write(output.as_path(), "built\n").unwrap();
        let writable = std::fs::metadata(output.as_path()).unwrap().permissions();
        let mut read_only = writable.clone();
        read_only.set_readonly(true);
        std::fs::set_permissions(output.as_path(), read_only).unwrap();
        let archive_path = dir.join("output.tar.zst");
        create_output_archive(&dir, &[path], &archive_path).unwrap();
        std::fs::set_permissions(output.as_path(), writable.clone()).unwrap();

        extract_output_archive(&dir, &archive_path).unwrap();

        let restored = std::fs::metadata(output.as_path()).unwrap().permissions();
        // Windows can't remove the directory while the file is read-only.
        std::fs::set_permissions(output.as_path(), writable).unwrap();
        assert!(restored.readonly());
    }
}
