#!/usr/bin/env python3
"""Measure pinned, published Vite outputs without installing or running projects.

Requires Python 3 and the zstd CLI. Usage:
  python3 measure.py --cache-dir /tmp/vite-cache-size-study --output results.json
"""

import argparse
import collections
import gzip
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import tarfile
import urllib.parse
import urllib.request


def read_url(url, headers=None):
    return urllib.request.urlopen(
        urllib.request.Request(
            url, headers={"User-Agent": "vite-task-cache-size-study", **(headers or {})}
        ),
        timeout=60,
    )


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def download(sample, cache):
    target = cache / (sample["id"] + ".tgz")
    if not target.exists() or sha256(target) != sample["sha256"]:
        headers = {}
        if sample.get("docker_repository"):
            query = urllib.parse.urlencode(
                {
                    "service": "registry.docker.io",
                    "scope": "repository:" + sample["docker_repository"] + ":pull",
                }
            )
            with read_url("https://auth.docker.io/token?" + query) as response:
                token = json.load(response)["token"]
            headers["Authorization"] = "Bearer " + token
        partial = target.with_suffix(".partial")
        with read_url(sample["url"], headers) as response, partial.open("wb") as output:
            shutil.copyfileobj(response, output)
        if sha256(partial) != sample["sha256"]:
            raise ValueError("Source digest mismatch: " + sample["id"])
        partial.replace(target)
    return target


def compress(source, members, destination):
    # Match the engine's ordinary zstd level (3). Normalize GNU tar headers,
    # sort paths, and include regular files only. No filesystem extraction.
    with destination.open("wb") as output:
        process = subprocess.Popen(
            ["zstd", "-3", "-T1", "-q", "-c"], stdin=subprocess.PIPE, stdout=output
        )
        try:
            with tarfile.open(fileobj=process.stdin, mode="w|", format=tarfile.GNU_FORMAT) as archive:
                for name, member in members:
                    header = tarfile.TarInfo("outputs/" + name)
                    header.size = member.size
                    header.mode = 0o644
                    with source.extractfile(member) as data:
                        archive.addfile(header, data)
        finally:
            process.stdin.close()
            status = process.wait()
        if status:
            raise RuntimeError("zstd failed: " + str(status))
    return {"bytes": destination.stat().st_size, "sha256": sha256(destination)}


def measure(sample, cache):
    packed = download(sample, cache)
    unpacked = cache / (sample["id"] + ".tar")
    # Decompress once so sorted reads do not repeatedly rewind a gzip stream.
    with gzip.open(packed, "rb") as source, unpacked.open("wb") as output:
        shutil.copyfileobj(source, output)

    with tarfile.open(unpacked) as source:
        prefix = sample["prefix"]
        members = []
        for member in source:
            if not member.name.startswith(prefix):
                continue
            name = member.name[len(prefix):]
            if member.isdir():
                continue
            if not member.isfile() or ".." in PurePosixPath(name).parts or name.startswith("/"):
                raise ValueError("Unsupported output member: " + member.name)
            members.append((name, member))
        members.sort(key=lambda item: item[0])
        if not members or len({name for name, _ in members}) != len(members):
            raise ValueError("Empty or duplicate output paths: " + sample["id"])

        by_extension = collections.defaultdict(lambda: {"files": 0, "bytes": 0})
        for name, member in members:
            group = by_extension[PurePosixPath(name).suffix or "(none)"]
            group["files"] += 1
            group["bytes"] += member.size
        without_maps = [(name, member) for name, member in members if not name.endswith(".map")]
        complete = compress(source, members, cache / (sample["id"] + ".tar.zst"))
        no_maps = (
            compress(source, without_maps, cache / (sample["id"] + "-no-maps.tar.zst"))
            if len(without_maps) != len(members)
            else complete.copy()
        )
        return {
            "id": sample["id"],
            "source_sha256": sample["sha256"],
            "source_download_bytes": packed.stat().st_size,
            "prefix": prefix,
            "files": len(members),
            "output_bytes": sum(member.size for _, member in members),
            "tar_zstd": complete,
            "without_maps": {
                "files": len(without_maps),
                "output_bytes": sum(member.size for _, member in without_maps),
                "tar_zstd": no_maps,
            },
            "by_extension": dict(sorted(by_extension.items())),
            "largest_files": [
                {"path": name, "bytes": member.size}
                for name, member in sorted(members, key=lambda item: item[1].size, reverse=True)[:5]
            ],
        }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cache-dir", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    samples = json.loads(Path(__file__).with_name("sources.json").read_text())
    args.cache_dir.mkdir(parents=True, exist_ok=True)
    results = []
    for sample in samples["samples"]:
        result = measure(sample, args.cache_dir)
        results.append(result)
        print(
            sample["id"],
            "files=" + str(result["files"]),
            "output_bytes=" + str(result["output_bytes"]),
            "zstd_bytes=" + str(result["tar_zstd"]["bytes"]),
            "without_maps_zstd_bytes=" + str(result["without_maps"]["tar_zstd"]["bytes"]),
            flush=True,
        )
    version = subprocess.check_output(["zstd", "--version"], text=True)
    args.output.write_text(
        json.dumps(
            {
                "measurement_date": samples["measurement_date"],
                "zstd_version": re.search(r"\bv(\d+\.\d+\.\d+)\b", version).group(1),
                "method": "Sorted GNU tar regular files; outputs/ prefix; mode 0644; uid/gid/mtime 0; zstd -3 -T1 stream",
                "samples": results,
            },
            indent=2,
        ) + "\n"
    )


if __name__ == "__main__":
    main()
