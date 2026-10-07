"""Download the pinned license generator, verifying its published SHA256."""
import hashlib
import os
from pathlib import Path
import platform
import tarfile
import urllib.request

VERSION = "0.8.4"
ASSETS = {
    "Windows": ("x86_64-pc-windows-msvc", "d5c38fb914bbad57c6a7d58c4847315bc3fe11efbe4fb51b3c515f997a56ceb7"),
    "Linux": ("x86_64-unknown-linux-musl", "c7381aa0cdc41fc0ee662cec8daa260da7817ad8ddea04cd4ddad425460adf14"),
}


def main():
    if platform.machine().lower() not in {"amd64", "x86_64"}:
        raise RuntimeError("Release tooling supports x86_64 only")
    target, digest = ASSETS[platform.system()]
    name = f"cargo-about-{VERSION}-{target}"
    directory = Path("target/release-tools")
    directory.mkdir(parents=True, exist_ok=True)
    archive = directory / f"{name}.tar.gz"
    urllib.request.urlretrieve(
        f"https://github.com/EmbarkStudios/cargo-about/releases/download/{VERSION}/{archive.name}", archive)
    if hashlib.sha256(archive.read_bytes()).hexdigest() != digest:
        raise RuntimeError("cargo-about checksum mismatch")
    # Python 3.12+ data filter rejects paths escaping the extraction directory.
    with tarfile.open(archive) as source:
        source.extractall(directory, filter="data")
    tool_directory = (directory / name).resolve()
    if output := os.environ.get("GITHUB_PATH"):
        with open(output, "a", encoding="utf-8") as file:
            file.write(f"{tool_directory}\n")
    print(tool_directory)


if __name__ == "__main__":
    main()
