"""Release configuration, license collection, and deterministic archive layout."""
import argparse
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import re
import subprocess
import tarfile
import time
import tomllib
from urllib.parse import urlsplit
import zipfile

ROOT = Path(__file__).resolve().parents[2]
DIST = ROOT / "dist"
PUBLIC_KEYS = ("JIGSALL_RENDEZVOUS_WSS_URL", "JIGSALL_ICE_STUN_SERVERS",
               "JIGSALL_ICE_ALLOW_PUBLIC_CANDIDATES")
GNS = ROOT / "vendor/game-networking-sockets-sys/thirdparty/GameNetworkingSockets"


def command(*arguments):
    return subprocess.check_output(arguments, cwd=ROOT, encoding="utf-8").strip()


def validate_defaults(values):
    if set(values) != set(PUBLIC_KEYS):
        raise ValueError("Only the three public Internet defaults may be embedded")
    if any("\n" in value or "\r" in value or "\0" in value for value in values.values()):
        raise ValueError("Internet defaults must be single-line values")
    endpoint = urlsplit(values[PUBLIC_KEYS[0]])
    if (endpoint.scheme != "wss" or not endpoint.hostname or endpoint.path != "/v1/ws"
            or endpoint.username or endpoint.password or endpoint.query or endpoint.fragment
            or endpoint.hostname.endswith(".example")):
        raise ValueError("Configure a public WSS endpoint ending in /v1/ws, without credentials")
    if values[PUBLIC_KEYS[2]] not in {"true", "false"}:
        raise ValueError("ICE_ALLOW_PUBLIC_CANDIDATES must be true or false")
    for address in values[PUBLIC_KEYS[1]].split(","):
        if not address.strip():
            continue
        if not re.fullmatch(r"stun:[a-zA-Z0-9.\[\]:-]+", address.strip()):
            raise ValueError("Only public stun: host/port addresses may be embedded")


def configure_defaults():
    values = {key: os.environ.get(key, "") for key in PUBLIC_KEYS}
    validate_defaults(values)
    (ROOT / "internet-defaults.env").write_text(
        "".join(f"{key}={values[key]}\n" for key in PUBLIC_KEYS), encoding="utf-8")


def verify_version(tag):
    manifests = [ROOT / "Cargo.toml", *(ROOT / package / "Cargo.toml" for package in ("core", "game", "puzzle", "ui"))]
    version = tomllib.loads(manifests[0].read_text(encoding="utf-8"))["workspace"]["package"]["version"]
    if tag != f"v{version}":
        raise ValueError("Release tag must equal v<workspace.package.version>")
    for manifest in manifests:
        if tomllib.loads(manifest.read_text(encoding="utf-8"))["package"]["version"] != {"workspace": True}:
            raise ValueError(f"Workspace version not inherited: {manifest}")
    lock = tomllib.loads((ROOT / "Cargo.lock").read_text(encoding="utf-8"))
    for package in lock["package"]:
        if package["name"] in {"jigsall", "jigsall-core", "jigsall-game", "jigsall-puzzle", "jigsall-ui"}:
            if package["version"] != version:
                raise ValueError("Workspace and lockfile versions differ")
    return version


def build():
    DIST.mkdir(exist_ok=True)
    native = None
    binary = None
    arguments = ["cargo", "build", "--locked", "--release", "-p", "jigsall", "--bin", "jigsall",
                 "--features", "rendezvous", "--message-format=json-render-diagnostics"]
    with subprocess.Popen(arguments, cwd=ROOT, stdout=subprocess.PIPE, encoding="utf-8") as process:
        for line in process.stdout:
            message = json.loads(line)
            if message.get("reason") == "build-script-executed" and "game-networking-sockets-sys" in message["package_id"]:
                native = message["out_dir"]
            if message.get("reason") == "compiler-artifact" and message.get("executable"):
                binary = message["executable"]
        if process.wait() != 0:
            raise subprocess.CalledProcessError(process.returncode, arguments)
    if not native or not binary:
        raise RuntimeError("Missing application or native build artifact")
    (DIST / "build.json").write_text(json.dumps({"binary": binary, "native": native, "features": "gns,rendezvous"}), encoding="utf-8")


def native_notices(native, system):
    sources = [(GNS / "LICENSE", "Valve GameNetworkingSockets 1.6.0"),
               (GNS / "src/external/vjson/LICENSE", "vjson"),
               (GNS / "src/external/sha1-wpa/README", "sha1-wpa"),
               (ROOT / "ui/fonts/OFL.txt", "M PLUS 1p"),
               (ROOT / "ui/fonts/README.md", "M PLUS 1p provenance")]
    # Preserve the exact embedded source notices, including Valve modifications.
    sections = ["Jigsall third-party native and font notices",
                "GNS Rust wrappers: game-networking-sockets and game-networking-sockets-sys 0.3.0.\n"
                "Authors: Hussein Ait-Lahcen; James M De Broeck.\n"
                "Declared MIT / Apache-2.0; license text is in THIRD_PARTY_LICENSES.txt.\n"
                "Local patches are described in docs/NETWORK_TRANSPORT.md.\n"
                "These notices do not assign a license to Jigsall itself."]
    for directory in ["curve25519-donna", "ed25519-donna"]:
        sources.append((GNS / f"src/external/{directory}/readme_VALVE.txt", directory + " Valve modifications"))
    header = GNS / "src/external/ed25519-donna/ed25519-donna.h"
    text = header.read_text(encoding="utf-8")
    sections.append(f"{header.relative_to(ROOT)}\n{text.split('*/', 1)[0]}*/")
    if system == "Windows":
        share = native / "vcpkg/installed/x64-windows/share"
        required = {"openssl", "protobuf", "abseil", "utf8-range"}
        found = {path.parent.name for path in share.glob("*/copyright")}
        if not required <= found:
            raise RuntimeError(f"Missing native license files: {sorted(required - found)}")
        sources.extend((path, f"vcpkg {path.parent.name}") for path in sorted(share.glob("*/copyright")))
        status = native / "vcpkg/installed/vcpkg/status"
        sources.append((status, "Native package versions (vcpkg status)"))
    elif system == "Linux":
        # The GNS build links the distro's OpenSSL/Protobuf archives. Record the
        # installed versions and their upstream/debian copyright terms.
        for package in ["libssl-dev", "libprotobuf-dev"]:
            sections.append(command("dpkg-query", "-W", "-f=${Package} ${Version}", package))
            sources.append((Path("/usr/share/doc") / package / "copyright", package))
        for path in sorted(Path("/usr/share/common-licenses").iterdir()):
            if path.is_file():
                sources.append((path, "Debian referenced license " + path.name))
    else:
        raise ValueError("Unsupported release OS")
    for path, label in sources:
        if not path.is_file():
            raise FileNotFoundError(path)
        sections.append(f"{label}\n{'=' * 79}\n{path.read_text(encoding='utf-8')}")
    return "\n\n".join(sections) + "\n"


def licenses(target, cargo_about):
    DIST.mkdir(exist_ok=True)
    subprocess.run([cargo_about, "generate", "--locked", "--features", "rendezvous", "--target", target,
                    "--fail", "-o", str(DIST / "THIRD_PARTY_LICENSES.txt"), "about.hbs"], cwd=ROOT, check=True)
    build_info = json.loads((DIST / "build.json").read_text(encoding="utf-8"))
    system = "Windows" if target == "x86_64-pc-windows-msvc" else "Linux"
    notices = native_notices(Path(build_info["native"]), system)
    (DIST / "THIRD_PARTY_NOTICES.txt").write_text(notices, encoding="utf-8")


def archive_payload(binary):
    files = {binary.name: binary.read_bytes()}
    for path in ["README.md", "ui/fonts/OFL.txt", "ui/fonts/README.md", "docs/PLAYING.md",
                 "docs/PRE_RELEASE_CHECKLIST.md", "docs/RELEASE_NOTES_TEMPLATE.md"]:
        name = path.replace("ui/fonts/", "licenses/MPLUS1p/")
        files[name] = (ROOT / path).read_bytes()
    for name in ["THIRD_PARTY_LICENSES.txt", "THIRD_PARTY_NOTICES.txt"]:
        files[name] = (DIST / name).read_bytes()
    # Include a project license only if the owner has supplied one.
    for path in sorted(ROOT.glob("LICENSE*")):
        if path.is_file():
            files[path.name] = path.read_bytes()
    version = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))["workspace"]["package"]["version"]
    build_info = json.loads((DIST / "build.json").read_text(encoding="utf-8"))
    files["BUILD_INFO.txt"] = (f"Jigsall {version}\ncommit={command('git', 'rev-parse', 'HEAD')}\n"
                               f"features={build_info['features']}\n").encode()
    files["SHA256SUMS"] = "".join(f"{hashlib.sha256(data).hexdigest()}  {name}\n"
                                  for name, data in sorted(files.items())).encode()
    return files


def write_archive(path, files, epoch, executable):
    if path.suffix == ".zip":
        # Fixed metadata makes repackaging the same inputs reproducible. This is
        # not a claim that compilation is bit-identical across hosts/toolchains.
        date = time.gmtime(max(epoch, 315532800))[:6]
        with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as archive:
            for name, data in sorted(files.items()):
                info = zipfile.ZipInfo("jigsall/" + name, date)
                info.create_system = 3
                info.external_attr = (0o100755 if name == executable else 0o100644) << 16
                info.compress_type = zipfile.ZIP_DEFLATED
                archive.writestr(info, data)
    else:
        with path.open("wb") as raw, gzip.GzipFile(filename="", fileobj=raw, mode="wb", mtime=epoch) as compressed:
            with tarfile.open(fileobj=compressed, mode="w", format=tarfile.USTAR_FORMAT) as archive:
                for name, data in sorted(files.items()):
                    info = tarfile.TarInfo("jigsall/" + name)
                    info.size = len(data)
                    info.mtime = epoch
                    info.mode = 0o755 if name == executable else 0o644
                    archive.addfile(info, io.BytesIO(data))


def package(name):
    info = json.loads((DIST / "build.json").read_text(encoding="utf-8"))
    binary = Path(info["binary"])
    epoch = int(command("git", "show", "-s", "--format=%ct", "HEAD"))
    write_archive(DIST / name, archive_payload(binary), epoch, binary.name)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="action", required=True)
    sub.add_parser("configure-defaults")
    sub.add_parser("build")
    sub.add_parser("verify-version").add_argument("tag")
    license_parser = sub.add_parser("licenses")
    license_parser.add_argument("--target", required=True, choices=["x86_64-pc-windows-msvc", "x86_64-unknown-linux-gnu"])
    license_parser.add_argument("--cargo-about", default="cargo-about")
    sub.add_parser("package").add_argument("name", choices=["jigsall-windows-x86_64.zip", "jigsall-linux-x86_64.tar.gz"])
    options = parser.parse_args()
    if options.action == "configure-defaults":
        configure_defaults()
    elif options.action == "verify-version":
        verify_version(options.tag)
    elif options.action == "build":
        build()
    elif options.action == "licenses":
        licenses(options.target, options.cargo_about)
    else:
        package(options.name)


if __name__ == "__main__":
    main()
