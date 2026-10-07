"""Cache identities and source timestamps for the hosted CI/release builds."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tarfile


ROOT = Path(__file__).resolve().parents[2]
SYS = "vendor/game-networking-sockets-sys"
TRIPLET = "x64-windows-static-md-release"
# A checkout must not make unchanged path dependencies newer than cached outputs.
# Content changes are handled by the sys tree SHA, manifests and lockfile key.
SOURCE_TIMESTAMP = 946684800  # 2000-01-01 UTC


def command(*args):
    return subprocess.check_output(args, cwd=ROOT, text=True, encoding="utf-8").strip()


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True).encode()).hexdigest()[:24]


def windows_toolchain():
    vswhere = Path(os.environ["ProgramFiles(x86)"]) / "Microsoft Visual Studio/Installer/vswhere.exe"
    installation = Path(command(str(vswhere), "-latest", "-products", "*", "-requires",
                                "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
                                "-property", "installationPath"))
    version = (installation / "VC/Auxiliary/Build/Microsoft.VCToolsVersion.default.txt").read_text().strip()
    compiler = installation / "VC/Tools/MSVC" / version / "bin/Hostx64/x64/cl.exe"
    sdk = Path(os.environ["ProgramFiles(x86)"]) / "Windows Kits/10/Include"
    return {"msvc": version, "compiler": hashlib.sha256(compiler.read_bytes()).hexdigest(),
            "sdk": sorted(path.name for path in sdk.glob("10.*"))}


def native_environment():
    versions = {"cmake": command("cmake", "--version"), "ninja": command("ninja", "--version")}
    if os.environ["RUNNER_OS"] == "Windows":
        versions.update(windows_toolchain())
        versions["llvm"] = "18.1.8"
    else:
        versions["c++"] = command("c++", "--version")
        versions["packages"] = command("dpkg-query", "-W", "libclang-18-dev", "libssl-dev",
                                       "libprotobuf-dev", "protobuf-compiler", "libc6-dev",
                                       "linux-libc-dev", "binutils")
    # Do not include ImageVersion or rustup's inventory of installed toolchains.
    # Preserve ABI/tool/configuration distinctions and absolute paths in CMake
    # caches and Cargo's build-script output without keying on unrelated tools.
    variables = ["CC", "CXX", "CFLAGS", "CXXFLAGS", "CPPFLAGS", "LDFLAGS",
                 "CMAKE_GENERATOR", "CMAKE_TOOLCHAIN_FILE", "LIBCLANG_PATH",
                 "GNS_VCPKG_BUILDTREES_ROOT", "CARGO_INCREMENTAL",
                 "CARGO_PROFILE_DEV_DEBUG", "CARGO_PROFILE_TEST_DEBUG",
                 "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS"]
    return {"versions": versions, "workspace": str(ROOT),
            "env": {name: os.environ.get(name, "") for name in variables}}


def normalize_sources(root=ROOT):
    files = subprocess.check_output(["git", "ls-files", "-z", "--", SYS], cwd=root).split(b"\0")
    directories = {root / SYS}
    for name in filter(None, files):
        path = root / os.fsdecode(name)
        os.utime(path, (SOURCE_TIMESTAMP, SOURCE_TIMESTAMP))
        directories.update(parent for parent in path.parents if parent.is_relative_to(root / SYS))
    # Cargo also checks the directories named by rerun-if-changed.
    for path in directories:
        os.utime(path, (SOURCE_TIMESTAMP, SOURCE_TIMESTAMP))


def cache_paths(profile):
    directory = "debug" if profile == "dev" else "release"
    prefix = f"target/{directory}"
    # build/ includes the executable AND the execution directory (output,
    # root-output, invoked.timestamp, stderr, and the native OUT_DIR).
    # The library target is named gns_sys, not game_networking_sockets_sys.
    return [f"{prefix}/build/game-networking-sockets-sys*",
            f"{prefix}/.fingerprint/game-networking-sockets-sys*",
            f"{prefix}/deps/gns_sys-*", f"{prefix}/deps/libgns_sys-*"]


def archive_filter(info):
    # Keep installed packages (linking + release notices), CMake outputs and
    # tooling. vcpkg's scratch package copies/downloads/history are redundant.
    parts = Path(info.name).parts
    for index in range(len(parts) - 2):
        if parts[index:index + 2] == ("GNS", "vcpkg") and parts[index + 2] in {
            ".git", "packages", "downloads", "buildtrees",
        }:
            return None
    return info


def pack(profile, archive):
    archive.parent.mkdir(parents=True, exist_ok=True)
    # Stage one uncompressed tar so the cache action's zstd archive can exclude
    # nested scratch directories without recursively caching their parents.
    # tar preserves Cargo/CMake timestamps and installed-package hardlinks.
    with tarfile.open(archive, "w") as output:
        for pattern in cache_paths(profile):
            for path in sorted(ROOT.glob(pattern)):
                output.add(path, arcname=path.relative_to(ROOT), filter=archive_filter)
    print(f"GNS {profile} snapshot: {archive.stat().st_size / 1024**2:.1f} MiB")


def unpack(archive):
    with tarfile.open(archive) as source:
        source.extractall(ROOT, filter="data")
    print("Restored GNS build output, fingerprints and library artifacts")


def prepare(profile):
    tree = command("git", "rev-parse", f"HEAD:{SYS}")
    environment = native_environment()
    outputs = {"gns-tree": tree, "native-environment": digest(environment), "triplet": TRIPLET,
               "cargo-inputs": os.environ["CARGO_CACHE_INPUTS"]}
    if os.environ["RUNNER_OS"] == "Windows":
        # Independent of Rust and Cargo profile, for CI/release package reuse.
        outputs["vcpkg-environment"] = digest(environment["versions"] | {"cmake": None, "ninja": None})
    normalize_sources()
    with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
        for name, value in outputs.items():
            output.write(f"{name}={value}\n")
    print(f"GNS {profile}: tree={tree}, native environment={outputs['native-environment']}")


def inspect(profile):
    paths = sorted({path for pattern in cache_paths(profile) for path in ROOT.glob(pattern)})
    count, size, has_output = 0, 0, False
    libraries = []
    for path in paths:
        groups = os.walk(path) if path.is_dir() else [(path.parent, [], [path.name])]
        for directory, directories, files in groups:
            if Path(directory).parts[-2:] == ("GNS", "vcpkg"):
                directories[:] = [name for name in directories if name not in {
                    ".git", "packages", "downloads", "buildtrees",
                }]
            for name in files:
                file = Path(directory) / name
                count += 1
                size += file.stat().st_size
                has_output |= name == "output"
                if name in ("GameNetworkingSockets_s.lib", "libGameNetworkingSockets_s.a"):
                    libraries.append(file)
    if not libraries or not has_output:
        raise RuntimeError("GNS cache is missing its native static library or Cargo build output")
    message = f"GNS cache: {count} files, {size / 1024**2:.1f} MiB"
    print(message)
    for path in libraries:
        print(f"Native library: {path.relative_to(ROOT)}")
    if summary := os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(summary, "a", encoding="utf-8") as output:
            output.write(f"\n{message}\n")


def summary():
    rows = []
    for label, variable in [("rust-cache", "RUST_CACHE_HIT"), ("vcpkg binary", "VCPKG_CACHE_HIT"),
                            ("GNS native", "GNS_CACHE_HIT")]:
        status = "hit (exact)" if os.environ.get(variable) == "true" else "miss"
        if label == "vcpkg binary" and os.environ["RUNNER_OS"] != "Windows":
            status = "not used on this OS"
        rows.append(f"| {label} | {status} |")
    report = "| Cache | Restore result |\n| --- | --- |\n" + "\n".join(rows) + "\n"
    print(report)
    with open(os.environ["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as output:
        output.write(report)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["prepare", "inspect", "summary", "pack", "unpack"])
    parser.add_argument("--profile", choices=["dev", "release"], default="dev")
    parser.add_argument("--archive", type=Path, default=os.environ.get("GNS_CACHE_ARCHIVE"))
    args = parser.parse_args()
    if args.action == "summary":
        summary()
    elif args.action in {"pack", "unpack"}:
        if args.archive is None:
            parser.error("--archive is required for pack/unpack")
        if args.action == "pack":
            inspect(args.profile)
            pack(args.profile, args.archive)
        else:
            unpack(args.archive)
    else:
        {"prepare": prepare, "inspect": inspect}[args.action](args.profile)
