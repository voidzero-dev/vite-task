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
    /// SHA-256 of the tarball at `url`. Each value can be obtained from the
    /// release download page.
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
                expected_sha256: "3a35f7ae2be85fcd32392cd8171522f5822f20a69125c5e9d8d68b2f5c857098",
            },
            // https://github.com/uutils/coreutils/releases/tag/0.4.0
            BinaryDownload {
                name: "coreutils",
                url: "https://github.com/uutils/coreutils/releases/download/0.4.0/coreutils-0.4.0-aarch64-apple-darwin.tar.gz",
                path_in_targz: "coreutils-0.4.0-aarch64-apple-darwin/coreutils",
                expected_sha256: "a148b660eeaf409af7a4406903f93d0e6713a5eb9adcaf71a1d732f1e3cc3522",
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
                expected_sha256: "aa12258d1bd553020144ad61fdac18e7dfbe3fc3965da32ee458840153169151",
            },
            // https://github.com/uutils/coreutils/releases/tag/0.4.0
            BinaryDownload {
                name: "coreutils",
                url: "https://github.com/uutils/coreutils/releases/download/0.4.0/coreutils-0.4.0-x86_64-apple-darwin.tar.gz",
                path_in_targz: "coreutils-0.4.0-x86_64-apple-darwin/coreutils",
                expected_sha256: "6e4be8429efe86c9a60247ae7a930221ed11770a975fb4b6fd09ff8d39b9a15c",
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
        // Reuse the extracted binary if it's already in OUT_DIR; the sha256
        // of the tarball was verified on the initial download. This avoids
        // hitting the network on incremental build-script reruns.
        if !dest.exists() {
            let tarball = download(url).context(format!("Failed to download {url}"))?;
            let actual_sha256 = sha256_hex(&tarball);
            assert_eq!(
                &actual_sha256, expected_sha256,
                "sha256 of {url} does not match — update expected value in MACOS_BINARY_DOWNLOADS",
            );
            let data = unpack_tar_gz(Cursor::new(tarball), path_in_targz)
                .context(format!("Failed to extract {path_in_targz} from {url}"))?;
            fs::write(&dest, &data).with_context(|| format!("writing {}", dest.display()))?;
        }
        bundled_artifact_build::register(name, &dest);
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
    bundled_artifact_build::register("fspy_preload", Path::new(&dylib_path));
    Ok(())
}

fn main() -> anyhow::Result<()> {
    println!("cargo:rerun-if-changed=build.rs");
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fetch_macos_binaries(&out_dir).context("Failed to fetch macOS binaries")?;
    register_preload_cdylib().context("Failed to register preload cdylib")?;
    Ok(())
}
