//! Find cache directories marked with a `CACHEDIR.TAG` file, as described by
//! the Cache Directory Tagging spec (<https://bford.info/cachedir/>).
//!
//! Reads inside a tagged directory are left out of automatic input tracking,
//! the same way as paths a tool reports through `ignoreInput`.
#![cfg(fspy)]

use std::io::Read as _;

use rustc_hash::FxHashSet;
use vt_path::{AbsolutePath, RelativePath, RelativePathBuf};

const TAG_FILE_NAME: &str = "CACHEDIR.TAG";
const SIGNATURE: &[u8; 43] = b"Signature: 8a477f597d28d172789f06886806bc55";

/// Return the directories that hold a valid `CACHEDIR.TAG` among `paths` and
/// their ancestors. Only directories inside the workspace are considered; the
/// workspace root itself is not.
///
/// Every directory is checked at most once, so the cost is bounded by the
/// number of distinct directories in `paths`.
pub fn find_tagged_dirs<'a>(
    paths: impl IntoIterator<Item = &'a RelativePathBuf>,
    workspace_root: &AbsolutePath,
) -> FxHashSet<RelativePathBuf> {
    let mut checked: FxHashSet<&'a RelativePath> = FxHashSet::default();
    let mut tagged = FxHashSet::default();
    for path in paths {
        if path.as_path().starts_with("..") {
            continue;
        }
        let mut dir: &'a RelativePath = path;
        // A checked directory's ancestors were checked along with it, so the
        // walk can stop at the first one already seen.
        while !dir.as_str().is_empty() && checked.insert(dir) {
            if has_valid_tag(&workspace_root.join(dir)) {
                tagged.insert(dir.to_relative_path_buf());
            }
            let Some(parent) = dir.parent() else { break };
            dir = parent;
        }
    }
    tagged
}

/// Whether `dir` contains a regular file named `CACHEDIR.TAG` that starts
/// with the spec's signature.
fn has_valid_tag(dir: &AbsolutePath) -> bool {
    let Ok(mut file) = std::fs::File::open(dir.join(TAG_FILE_NAME)) else {
        return false;
    };
    if !file.metadata().is_ok_and(|metadata| metadata.is_file()) {
        return false;
    }
    let mut header = [0u8; SIGNATURE.len()];
    file.read_exact(&mut header).is_ok() && &header == SIGNATURE
}

#[cfg(test)]
mod tests {
    use vt_path::AbsolutePathBuf;

    use super::*;

    fn rel(path: &str) -> RelativePathBuf {
        RelativePathBuf::new(path).unwrap()
    }

    fn write(root: &AbsolutePath, path: &str, content: &[u8]) {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn finds_tagged_ancestors_and_rejects_invalid_tags() {
        let temp = tempfile::tempdir().unwrap();
        let root = AbsolutePathBuf::new(temp.path().canonicalize().unwrap()).unwrap();

        let mut valid_tag = SIGNATURE.to_vec();
        valid_tag.extend_from_slice(b"\n# This file is a cache directory tag.\n");
        write(&root, "valid/CACHEDIR.TAG", &valid_tag);
        write(&root, "valid/sub/file.txt", b"");
        write(&root, "exact/CACHEDIR.TAG", SIGNATURE);
        write(
            &root,
            "wrong-signature/CACHEDIR.TAG",
            b"Signature: 00000000000000000000000000000000",
        );
        write(&root, "wrong-signature/file.txt", b"");
        write(&root, "truncated/CACHEDIR.TAG", &SIGNATURE[..42]);
        write(&root, "truncated/file.txt", b"");
        write(&root, "tag-is-dir/CACHEDIR.TAG/file.txt", &valid_tag);
        write(&root, "untagged/file.txt", b"");

        let paths = [
            rel("valid/sub/file.txt"),
            rel("valid/sub/other.txt"),
            rel("exact"),
            rel("wrong-signature/file.txt"),
            rel("truncated/file.txt"),
            rel("tag-is-dir/CACHEDIR.TAG/file.txt"),
            rel("untagged/file.txt"),
        ];
        let tagged = find_tagged_dirs(&paths, &root);

        let expected: FxHashSet<RelativePathBuf> =
            [rel("valid"), rel("exact")].into_iter().collect();
        assert_eq!(tagged, expected);
    }
}
