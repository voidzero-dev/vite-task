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
/// Parent directories are created automatically. Existing files are replaced
/// atomically, so concurrent extractions of the same archive into one
/// workspace don't fail and never expose a partly written file.
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

    for entry in archive.entries()? {
        let mut entry = entry?;
        if entry.header().entry_type().is_file() {
            unpack_file_atomically(&mut entry, workspace_root)?;
        } else {
            entry.unpack_in(workspace_root.as_path())?;
        }
    }
    Ok(())
}

/// Unpack a regular file entry by writing a temporary file next to the target
/// and renaming it over the target.
///
/// `tar` removes an existing target and then creates it with `create_new`, so
/// two extractions of the same file can make one of them fail with
/// `File exists`. The rename can't fail that way, and whichever extraction
/// renames last wins with identical bytes.
fn unpack_file_atomically(
    entry: &mut tar::Entry<'_, impl Read>,
    workspace_root: &AbsolutePath,
) -> anyhow::Result<()> {
    let rel_path = entry.path()?.into_owned();
    if !rel_path.components().all(|component| matches!(component, Component::Normal(_))) {
        return Err(anyhow::Error::msg(vt_str::format!(
            "archive entry `{}` is not a path inside the workspace",
            rel_path.display(),
        )));
    }
    let target = workspace_root.as_path().join(&rel_path);
    let parent = target.parent().expect("a joined entry path has a parent");
    // Like `Entry::unpack_in`, refuse entries that resolve outside the workspace
    // through a symlinked directory, and check before creating missing directories.
    let canonical_root = workspace_root.as_path().canonicalize()?;
    #[expect(clippy::disallowed_types, reason = "checks std paths at the filesystem boundary")]
    let ensure_inside_workspace = |dir: &std::path::Path| -> anyhow::Result<()> {
        if dir.canonicalize()?.starts_with(&canonical_root) {
            return Ok(());
        }
        Err(anyhow::Error::msg(vt_str::format!(
            "archive entry `{}` resolves outside the workspace",
            rel_path.display(),
        )))
    };
    let mut deepest_existing = parent;
    while deepest_existing.symlink_metadata().is_err()
        && let Some(ancestor) = deepest_existing.parent()
    {
        deepest_existing = ancestor;
    }
    ensure_inside_workspace(deepest_existing)?;
    std::fs::create_dir_all(parent)?;
    ensure_inside_workspace(parent)?;
    // The temporary path is removed on drop, so a failed unpack or rename leaves nothing behind.
    // Plain `fs::rename` rather than `TempPath::persist`. On Windows both call `MoveFileExW`,
    // which fails with access denied when the target is open or being replaced at the same
    // time, but only std then retries with a POSIX-semantics rename. That replaces a target
    // a reader has open with `FILE_SHARE_DELETE`, which std passes for every file it opens.
    let temp_path = tempfile::NamedTempFile::new_in(parent)?.into_temp_path();
    entry.unpack(&temp_path)?;
    std::fs::rename(&temp_path, &target)?;
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
    use std::{
        sync::{
            Barrier,
            atomic::{AtomicBool, Ordering},
        },
        thread,
    };

    use vt_path::AbsolutePathBuf;

    use super::*;

    #[test]
    fn concurrent_extractions_into_one_workspace_succeed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = AbsolutePathBuf::new(tmp.path().to_path_buf()).unwrap();
        let outputs: Vec<RelativePathBuf> = (0..20)
            .map(|i| RelativePathBuf::new(vt_str::format!("dist/sub/out{i}.txt")).unwrap())
            .collect();
        for (i, output) in outputs.iter().enumerate() {
            let path = dir.join(output);
            std::fs::create_dir_all(path.as_path().parent().unwrap()).unwrap();
            std::fs::write(path.as_path(), vt_str::format!("content {i}\n").repeat(2000)).unwrap();
        }
        let archive_path = dir.join("output.tar.zst");
        create_output_archive(&dir, &outputs, &archive_path).unwrap();

        let workspace = AbsolutePathBuf::new(tmp.path().join("workspace")).unwrap();
        std::fs::create_dir(workspace.as_path()).unwrap();
        for _ in 0..50 {
            let barrier = Barrier::new(2);
            thread::scope(|scope| {
                for _ in 0..2 {
                    scope.spawn(|| {
                        barrier.wait();
                        extract_output_archive(&workspace, &archive_path).unwrap();
                    });
                }
            });
            for (i, output) in outputs.iter().enumerate() {
                assert_eq!(
                    std::fs::read_to_string(workspace.join(output).as_path()).unwrap(),
                    vt_str::format!("content {i}\n").repeat(2000),
                );
            }
        }
    }

    #[test]
    fn extraction_replaces_existing_files_and_leaves_nothing_behind_on_failure() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = AbsolutePathBuf::new(tmp.path().to_path_buf()).unwrap();
        let output = RelativePathBuf::new("dist/out.txt").unwrap();
        let path = dir.join(&output);
        std::fs::create_dir_all(path.as_path().parent().unwrap()).unwrap();
        // Incompressible content, so that truncating the archive cuts into the entry.
        let content: Vec<u8> =
            (0..100_000_u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8).collect();
        std::fs::write(path.as_path(), &content).unwrap();
        let archive_path = dir.join("output.tar.zst");
        create_output_archive(&dir, &[output], &archive_path).unwrap();

        let workspace = AbsolutePathBuf::new(tmp.path().join("workspace")).unwrap();
        let restored = workspace.join("dist/out.txt");
        std::fs::create_dir_all(restored.as_path().parent().unwrap()).unwrap();
        std::fs::write(restored.as_path(), "stale").unwrap();
        extract_output_archive(&workspace, &archive_path).unwrap();
        assert_eq!(std::fs::read(restored.as_path()).unwrap(), content);

        let archive = std::fs::read(archive_path.as_path()).unwrap();
        let truncated = dir.join("truncated.tar.zst");
        std::fs::write(truncated.as_path(), &archive[..archive.len() / 2]).unwrap();
        let fresh = AbsolutePathBuf::new(tmp.path().join("fresh")).unwrap();
        std::fs::create_dir(fresh.as_path()).unwrap();
        assert!(extract_output_archive(&fresh, &truncated).is_err());
        // Neither the target nor a temporary file may be left behind.
        let left_behind =
            std::fs::read_dir(fresh.join("dist").as_path()).map_or(0, Iterator::count);
        assert_eq!(left_behind, 0);
    }

    #[test]
    fn failed_rename_leaves_no_temporary_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = AbsolutePathBuf::new(tmp.path().to_path_buf()).unwrap();
        let output = RelativePathBuf::new("dist/out.txt").unwrap();
        let path = dir.join(&output);
        std::fs::create_dir_all(path.as_path().parent().unwrap()).unwrap();
        std::fs::write(path.as_path(), "built\n").unwrap();
        let archive_path = dir.join("output.tar.zst");
        create_output_archive(&dir, &[output], &archive_path).unwrap();

        // A non-empty directory at the target makes the final rename fail.
        let workspace = AbsolutePathBuf::new(tmp.path().join("workspace")).unwrap();
        let blocker = workspace.join("dist/out.txt/child");
        std::fs::create_dir_all(blocker.as_path()).unwrap();
        assert!(extract_output_archive(&workspace, &archive_path).is_err());
        let names: Vec<_> = std::fs::read_dir(workspace.join("dist").as_path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["out.txt"]);
    }

    #[test]
    fn readers_never_see_a_missing_or_partial_file_while_restores_run() {
        const FILES: usize = 8;
        let expected = |i: usize| vt_str::format!("content {i}\n").repeat(20_000);

        let tmp = tempfile::TempDir::new().unwrap();
        let dir = AbsolutePathBuf::new(tmp.path().to_path_buf()).unwrap();
        let outputs: Vec<RelativePathBuf> = (0..FILES)
            .map(|i| RelativePathBuf::new(vt_str::format!("dist/out{i}.txt")).unwrap())
            .collect();
        for (i, output) in outputs.iter().enumerate() {
            let path = dir.join(output);
            std::fs::create_dir_all(path.as_path().parent().unwrap()).unwrap();
            std::fs::write(path.as_path(), expected(i)).unwrap();
        }
        let archive_path = dir.join("output.tar.zst");
        create_output_archive(&dir, &outputs, &archive_path).unwrap();

        let workspace = AbsolutePathBuf::new(tmp.path().join("workspace")).unwrap();
        std::fs::create_dir(workspace.as_path()).unwrap();
        // The files exist before the readers start, as after an earlier cache hit.
        extract_output_archive(&workspace, &archive_path).unwrap();

        let done = AtomicBool::new(false);
        thread::scope(|scope| {
            let readers: Vec<_> = (0..2)
                .map(|_| {
                    scope.spawn(|| {
                        let mut reads = 0_u32;
                        // Read at least once more after the restores finished.
                        loop {
                            let finished = done.load(Ordering::SeqCst);
                            for (i, output) in outputs.iter().enumerate() {
                                // `fs::read` fails if the file is missing.
                                let bytes = std::fs::read(workspace.join(output).as_path())
                                    .unwrap_or_else(|err| panic!("{output}: {err}"));
                                assert!(bytes == expected(i).as_bytes(), "{output}: partial file");
                                reads += 1;
                            }
                            if finished {
                                return reads;
                            }
                        }
                    })
                })
                .collect();
            let restorers: Vec<_> = (0..2)
                .map(|_| {
                    scope.spawn(|| {
                        for _ in 0..30 {
                            extract_output_archive(&workspace, &archive_path).unwrap();
                        }
                    })
                })
                .collect();
            for restorer in restorers {
                restorer.join().unwrap();
            }
            done.store(true, Ordering::SeqCst);
            for reader in readers {
                assert!(reader.join().unwrap() > 0);
            }
        });
    }

    /// A tar.zst archive with one regular file entry whose path is written to
    /// the header verbatim (`tar::Builder` itself refuses `..`).
    fn archive_with_raw_path(path: &str) -> Vec<u8> {
        let mut header = tar::Header::new_gnu();
        header.as_old_mut().name[..path.len()].copy_from_slice(path.as_bytes());
        header.set_size(5);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_cksum();
        let mut builder = tar::Builder::new(Vec::new());
        builder.append(&header, &b"evil\n"[..]).unwrap();
        zstd::encode_all(builder.into_inner().unwrap().as_slice(), 0).unwrap()
    }

    #[test]
    fn entries_outside_the_workspace_are_rejected() {
        let tmp = tempfile::TempDir::new().unwrap();
        let workspace = AbsolutePathBuf::new(tmp.path().join("workspace")).unwrap();
        std::fs::create_dir(workspace.as_path()).unwrap();
        let outside = tmp.path().join("evil.txt");
        let absolute = outside.to_str().unwrap().to_owned();

        for (i, path) in
            ["../evil.txt", "dist/../../evil.txt", absolute.as_str()].into_iter().enumerate()
        {
            let archive_path =
                AbsolutePathBuf::new(tmp.path().join(vt_str::format!("evil{i}.tar.zst").as_str()))
                    .unwrap();
            std::fs::write(archive_path.as_path(), archive_with_raw_path(path)).unwrap();
            assert!(
                extract_output_archive(&workspace, &archive_path).is_err(),
                "`{path}` must be rejected",
            );
            assert!(!outside.exists(), "`{path}` escaped the workspace");
        }
    }

    #[test]
    fn entries_through_a_symlinked_directory_out_of_the_workspace_are_rejected() {
        let tmp = tempfile::TempDir::new().unwrap();
        let workspace = AbsolutePathBuf::new(tmp.path().join("workspace")).unwrap();
        let outside = tmp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::create_dir(workspace.as_path()).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, workspace.join("dist").as_path()).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&outside, workspace.join("dist").as_path()).unwrap();

        let archive_path = AbsolutePathBuf::new(tmp.path().join("output.tar.zst")).unwrap();
        // Neither the file nor the missing `sub` directory may be created outside.
        for path in ["dist/evil.txt", "dist/sub/evil.txt"] {
            std::fs::write(archive_path.as_path(), archive_with_raw_path(path)).unwrap();
            assert!(extract_output_archive(&workspace, &archive_path).is_err(), "{path}");
            assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0, "{path}");
        }
    }

    #[test]
    fn restored_files_keep_their_modification_time() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = AbsolutePathBuf::new(tmp.path().to_path_buf()).unwrap();
        let output = RelativePathBuf::new("dist/out.txt").unwrap();
        let path = dir.join(&output);
        std::fs::create_dir_all(path.as_path().parent().unwrap()).unwrap();
        std::fs::write(path.as_path(), "built\n").unwrap();
        // A whole second in the past, as tar stores mtime in seconds.
        let mtime = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000);
        File::options().write(true).open(path.as_path()).unwrap().set_modified(mtime).unwrap();
        let archive_path = dir.join("output.tar.zst");
        create_output_archive(&dir, &[output], &archive_path).unwrap();

        let workspace = AbsolutePathBuf::new(tmp.path().join("workspace")).unwrap();
        std::fs::create_dir(workspace.as_path()).unwrap();
        extract_output_archive(&workspace, &archive_path).unwrap();
        let restored = std::fs::metadata(workspace.join("dist/out.txt").as_path()).unwrap();
        assert_eq!(restored.modified().unwrap(), mtime);
    }

    #[cfg(unix)]
    #[test]
    fn restored_files_keep_their_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::TempDir::new().unwrap();
        let dir = AbsolutePathBuf::new(tmp.path().to_path_buf()).unwrap();
        let modes = [("dist/run.sh", 0o755), ("dist/data.txt", 0o644), ("dist/private.txt", 0o600)];
        let outputs: Vec<RelativePathBuf> =
            modes.iter().map(|(name, _)| RelativePathBuf::new(*name).unwrap()).collect();
        std::fs::create_dir(dir.join("dist").as_path()).unwrap();
        for ((name, mode), output) in modes.iter().zip(&outputs) {
            let path = dir.join(output);
            std::fs::write(path.as_path(), name).unwrap();
            std::fs::set_permissions(path.as_path(), std::fs::Permissions::from_mode(*mode))
                .unwrap();
        }
        let archive_path = dir.join("output.tar.zst");
        create_output_archive(&dir, &outputs, &archive_path).unwrap();

        let workspace = AbsolutePathBuf::new(tmp.path().join("workspace")).unwrap();
        std::fs::create_dir(workspace.as_path()).unwrap();
        // Extract twice: the second run replaces files that already exist.
        for _ in 0..2 {
            extract_output_archive(&workspace, &archive_path).unwrap();
            for (name, mode) in modes {
                let restored = std::fs::metadata(workspace.join(name).as_path()).unwrap();
                assert_eq!(restored.permissions().mode() & 0o777, mode, "{name}");
            }
        }
    }

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
}
