use std::{
    env,
    fmt::Write as _,
    fs,
    io::{Cursor, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{Context, bail};
use sha2::{Digest, Sha256};

fn download(url: &str) -> anyhow::Result<Vec<u8>> {
    let curl = Command::new("curl")
        .args([
            "-f", // fail on HTTP errors
            "-L", // follow redirects
            url,
        ])
        .stdout(Stdio::piped())
        .spawn()?;
    let output = curl.wait_with_output()?;
    if !output.status.success() {
        bail!("curl exited with status {} trying to download {}", output.status, url);
    }
    Ok(output.stdout)
}

fn unpack_tar_gz(tarball: impl Read, path: &str) -> anyhow::Result<Vec<u8>> {
    use flate2::read::GzDecoder;
    use tar::Archive;

    let tar = GzDecoder::new(tarball);
    let mut archive = Archive::new(tar);
    for entry in archive.entries()? {
        let mut entry = entry?;
        if entry.path_bytes().as_ref() == path.as_bytes() {
            let mut data = Vec::<u8>::with_capacity(entry.size().try_into().unwrap());
            entry.read_to_end(&mut data)?;
            return Ok(data);
        }
    }
    bail!("Path {path} not found in tar gz")
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut s = String::with_capacity(64);
    for b in digest {
        write!(&mut s, "{b:02x}").unwrap();
    }
    s
}

struct BinaryDownload {
    /// Identifier used both as the on-disk filename in `OUT_DIR` and as the
    /// env-var prefix consumed by `artifact!($name)` at runtime.
    name: &'static str,
    /// GitHub release asset URL.
    url: &'static str,
    /// Path of the binary within the tarball.
    path_in_targz: &'static str,
    /// SHA-256 of the extracted binary. Doubles as the cache key: an
    /// already-extracted binary in `OUT_DIR` whose content hashes to this
    /// value is reused without hitting the network.
    expected_sha256: &'static str,
}

const MACOS_BINARY_DOWNLOADS: &[(&str, &[BinaryDownload])] = &[
    (
        "aarch64",
        &[
            // https://github.com/branchseer/oils-for-unix-build/releases/tag/oils-for-unix-0.37.0
            BinaryDownload {
                name: "oils_for_unix",
                url: "https://github.com/branchseer/oils-for-unix-build/releases/download/oils-for-unix-0.37.0/oils-for-unix-0.37.0-darwin-arm64.tar.gz",
                path_in_targz: "oils-for-unix",
                expected_sha256: "ce4bb80b15f0a0371af08b19b65bfa5ea17d30429ebb911f487de3d2bcc7a07d",
            },
            // https://github.com/uutils/coreutils/releases/tag/0.4.0
            BinaryDownload {
                name: "coreutils",
                url: "https://github.com/uutils/coreutils/releases/download/0.4.0/coreutils-0.4.0-aarch64-apple-darwin.tar.gz",
                path_in_targz: "coreutils-0.4.0-aarch64-apple-darwin/coreutils",
                expected_sha256: "8e8f38d9323135a19a73d617336fce85380f3c46fcb83d3ae3e031d1c0372f21",
            },
        ],
    ),
    (
        "x86_64",
        &[
            // https://github.com/branchseer/oils-for-unix-build/releases/tag/oils-for-unix-0.37.0
            BinaryDownload {
                name: "oils_for_unix",
                url: "https://github.com/branchseer/oils-for-unix-build/releases/download/oils-for-unix-0.37.0/oils-for-unix-0.37.0-darwin-x86_64.tar.gz",
                path_in_targz: "oils-for-unix",
                expected_sha256: "cf1a95993127770e2a5fff277cd256a2bb28cf97d7f83ae42fdccc172cdb540d",
            },
            // https://github.com/uutils/coreutils/releases/tag/0.4.0
            BinaryDownload {
                name: "coreutils",
                url: "https://github.com/uutils/coreutils/releases/download/0.4.0/coreutils-0.4.0-x86_64-apple-darwin.tar.gz",
                path_in_targz: "coreutils-0.4.0-x86_64-apple-darwin/coreutils",
                expected_sha256: "6be8bee6e8b91fc44a465203b9cc30538af00084b6657dc136d9e55837753eb1",
            },
        ],
    ),
];

fn fetch_macos_binaries(out_dir: &Path) -> anyhow::Result<()> {
    if env::var("CARGO_CFG_TARGET_OS").unwrap() != "macos" {
        return Ok(());
    }

    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    let downloads = MACOS_BINARY_DOWNLOADS
        .iter()
        .find(|(arch, _)| *arch == target_arch)
        .context(format!("Unsupported macOS arch: {target_arch}"))?
        .1;

    for BinaryDownload { name, url, path_in_targz, expected_sha256 } in downloads {
        let dest = out_dir.join(name);
        // Cache hit: an already-extracted binary whose contents hash to
        // `expected_sha256` is known-good and reused without redownloading.
        let cached = matches!(
            fs::read(&dest),
            Ok(existing) if sha256_hex(&existing) == *expected_sha256,
        );
        if !cached {
            let tarball = download(url).context(format!("Failed to download {url}"))?;
            let data = unpack_tar_gz(Cursor::new(tarball), path_in_targz)
                .context(format!("Failed to extract {path_in_targz} from {url}"))?;
            let actual_sha256 = sha256_hex(&data);
            assert_eq!(
                &actual_sha256, expected_sha256,
                "sha256 of {path_in_targz} in {url} does not match — update expected value in MACOS_BINARY_DOWNLOADS",
            );
            fs::write(&dest, &data).with_context(|| format!("writing {}", dest.display()))?;
        }
        materialized_artifact_build::register(name, &dest);
    }
    Ok(())
}

fn register_preload_cdylib() -> anyhow::Result<()> {
    let env_name = match env::var("CARGO_CFG_TARGET_OS").unwrap().as_str() {
        "windows" => "CARGO_CDYLIB_FILE_FSPY_PRELOAD_WINDOWS",
        _ if env::var("CARGO_CFG_TARGET_ENV").unwrap() == "musl" => return Ok(()),
        _ => "CARGO_CDYLIB_FILE_FSPY_PRELOAD_UNIX",
    };
    // The cdylib path is content-addressed by cargo; when its content changes
    // the path changes. Track it so we re-publish the hash on update.
    println!("cargo:rerun-if-env-changed={env_name}");
    let dylib_path = env::var_os(env_name).with_context(|| format!("{env_name} not set"))?;
    materialized_artifact_build::register("fspy_preload", Path::new(&dylib_path));
    Ok(())
}

fn main() -> anyhow::Result<()> {
    println!("cargo:rerun-if-changed=build.rs");
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fetch_macos_binaries(&out_dir).context("Failed to fetch macOS binaries")?;
    register_preload_cdylib().context("Failed to register preload cdylib")?;
    Ok(())
}
