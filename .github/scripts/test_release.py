"""Regression checks for public configuration and distributable archives."""
import hashlib
import io
from pathlib import Path
import tarfile
import tempfile
import tomllib
import unittest
import zipfile

import release


class ReleaseTests(unittest.TestCase):
    def defaults(self):
        return dict(zip(release.PUBLIC_KEYS, ["wss://rendezvous.example.org/v1/ws", "stun:stun.example.org:3478", "true"]))

    def test_defaults_reject_credentials_placeholders_and_extra_keys(self):
        release.validate_defaults(self.defaults())
        for endpoint in ["", "ws://localhost/v1/ws", "wss://your-rendezvous.example/v1/ws",
                         "wss://user:password@server.org/v1/ws", "wss://server.org/v1/ws?token=secret",
                         "wss://server.org/v1/ws\nSECRET=bad"]:
            values = self.defaults()
            values[release.PUBLIC_KEYS[0]] = endpoint
            with self.assertRaises(ValueError):
                release.validate_defaults(values)
        for key, value in [("TURN_PASSWORD", "secret"), (release.PUBLIC_KEYS[1], "turn:user:password@server"),
                           (release.PUBLIC_KEYS[2], "yes")]:
            values = self.defaults()
            values[key] = value
            with self.assertRaises(ValueError):
                release.validate_defaults(values)

    def test_version_tag_is_exact(self):
        version = tomllib.loads((release.ROOT / "Cargo.toml").read_text(encoding="utf-8"))["workspace"]["package"]["version"]
        self.assertEqual(release.verify_version(f"v{version}"), version)
        for tag in ["v999.0.0", f"v{version}-different", version]:
            with self.assertRaises(ValueError):
                release.verify_version(tag)

    def test_archive_layout_checksums_permissions_and_repeatability(self):
        files = {"jigsall": b"test executable", "README.md": b"readme", "licenses/font.txt": b"notice"}
        files["SHA256SUMS"] = "".join(f"{hashlib.sha256(data).hexdigest()}  {name}\n"
                                      for name, data in sorted(files.items())).encode()
        with tempfile.TemporaryDirectory() as directory:
            for suffix in ["zip", "tar.gz"]:
                path = Path(directory) / f"release.{suffix}"
                release.write_archive(path, files, 1728000000, "jigsall")
                original = path.read_bytes()
                release.write_archive(path, files, 1728000000, "jigsall")
                self.assertEqual(path.read_bytes(), original)
                if suffix == "zip":
                    with zipfile.ZipFile(io.BytesIO(original)) as archive:
                        contents = {name.removeprefix("jigsall/"): archive.read(name) for name in archive.namelist()}
                else:
                    with tarfile.open(fileobj=io.BytesIO(original), mode="r:gz") as archive:
                        self.assertEqual(archive.getmember("jigsall/jigsall").mode, 0o755)
                        contents = {item.name.removeprefix("jigsall/"): archive.extractfile(item).read() for item in archive}
                self.assertEqual(contents, files)
                for line in contents["SHA256SUMS"].decode().splitlines():
                    digest, name = line.split("  ", 1)
                    self.assertEqual(hashlib.sha256(contents[name]).hexdigest(), digest)

    def test_native_notices_fail_on_missing_built_dependency(self):
        with tempfile.TemporaryDirectory() as directory:
            native = Path(directory)
            with self.assertRaises(RuntimeError):
                release.native_notices(native, "Windows")
            for name in ["openssl", "protobuf", "abseil", "utf8-range"]:
                path = native / f"vcpkg/installed/x64-windows-static-md-release/share/{name}/copyright"
                path.parent.mkdir(parents=True)
                path.write_text(f"{name} exact installed license", encoding="utf-8")
            status = native / "vcpkg/installed/vcpkg/status"
            status.parent.mkdir(parents=True)
            status.write_text("Package: protobuf\nVersion: test\n", encoding="utf-8")
            notices = release.native_notices(native, "Windows")
            for text in ["protobuf exact installed license", "Version: test", "Valve Corporation", "M PLUS 1p"]:
                self.assertIn(text, notices)


if __name__ == "__main__":
    unittest.main()
