"""Regression checks for the narrow GNS cache and checkout timestamps."""

import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import build_cache


class BuildCacheTests(unittest.TestCase):
    def test_native_identity_ignores_unselected_rust_but_tracks_native_flags(self):
        with patch.dict(os.environ, {"RUNNER_OS": "Linux"}, clear=True), \
                patch.object(build_cache, "command", return_value="fixed native tool version"):
            original = build_cache.digest(build_cache.native_environment())
            with patch.dict(os.environ, {"RUSTUP_TOOLCHAIN": "1.98.1", "ImageVersion": "new-image"}):
                self.assertEqual(build_cache.digest(build_cache.native_environment()), original)
            with patch.dict(os.environ, {"CXXFLAGS": "-march=native"}):
                self.assertNotEqual(build_cache.digest(build_cache.native_environment()), original)

    def test_cache_roundtrip_keeps_native_and_cargo_freshness_inputs_only(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            expected = [
                "build/game-networking-sockets-sys-aaaa/build-script-build.exe",
                "build/game-networking-sockets-sys-bbbb/output",
                "build/game-networking-sockets-sys-bbbb/root-output",
                "build/game-networking-sockets-sys-bbbb/invoked.timestamp",
                "build/game-networking-sockets-sys-bbbb/out/bindings.rs",
                "build/game-networking-sockets-sys-bbbb/out/build/CMakeCache.txt",
                "build/game-networking-sockets-sys-bbbb/out/lib/GameNetworkingSockets_s.lib",
                "build/game-networking-sockets-sys-bbbb/out/vcpkg/installed/vcpkg/status",
                ".fingerprint/game-networking-sockets-sys-aaaa/dep-build-script-build-script-build",
                ".fingerprint/game-networking-sockets-sys-bbbb/run-build-script-build-script-build.json",
                ".fingerprint/game-networking-sockets-sys-cccc/dep-lib-gns_sys",
                "deps/libgns_sys-cccc.rlib", "deps/libgns_sys-cccc.rmeta", "deps/gns_sys-cccc.d",
            ]
            for profile in ["debug", "release"]:
                for name in [*expected, "deps/libbevy-aaaa.rlib", "build/other-sys-aaaa/out/lib.a",
                             "build/game-networking-sockets-sys-bbbb/out/GNS/vcpkg/.git/config",
                             "build/game-networking-sockets-sys-bbbb/out/GNS/vcpkg/packages/protobuf/lib.a",
                             "build/game-networking-sockets-sys-bbbb/out/GNS/vcpkg/downloads/package.zip"]:
                    path = root / "target" / profile / name
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_text(name)
            archive = root / "cache.tar"
            with patch.object(build_cache, "ROOT", root):
                build_cache.pack("dev", archive)
            restored = root / "restored"
            with patch.object(build_cache, "ROOT", restored):
                build_cache.unpack(archive)
            actual = {str(path.relative_to(restored)).replace("\\", "/")
                      for path in restored.rglob("*") if path.is_file()}
            self.assertEqual(actual, {f"target/debug/{name}" for name in expected})
            with patch.object(build_cache, "ROOT", restored):
                build_cache.inspect("dev")
                with self.assertRaises(RuntimeError):
                    build_cache.inspect("release")

    def test_checkout_timestamps_are_stable_without_changing_content(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            names = [f"{build_cache.SYS}/src/lib.rs", f"{build_cache.SYS}/thirdparty/GNS/src/code.cpp"]
            for name in [*names, "Cargo.toml"]:
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(name)
            untouched = (root / "Cargo.toml").stat().st_mtime_ns
            with patch.object(build_cache.subprocess, "check_output", return_value="\0".join(names).encode()):
                build_cache.normalize_sources(root)
            for name in names:
                path = root / name
                self.assertEqual(path.read_text(), name)
                self.assertEqual(path.stat().st_mtime, build_cache.SOURCE_TIMESTAMP)
                self.assertEqual(path.parent.stat().st_mtime, build_cache.SOURCE_TIMESTAMP)
            self.assertEqual((root / "Cargo.toml").stat().st_mtime_ns, untouched)

    def test_summary_distinguishes_exact_hits_misses_and_unused_cache(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "summary"
            with patch.dict(os.environ, {"GITHUB_STEP_SUMMARY": str(path), "RUNNER_OS": "Windows",
                                         "RUST_CACHE_HIT": "true", "VCPKG_CACHE_HIT": "false",
                                         "GNS_CACHE_HIT": ""}):
                build_cache.summary()
            report = path.read_text()
            self.assertIn("| rust-cache | hit (exact) |", report)
            self.assertIn("| vcpkg binary | miss |", report)
            self.assertIn("| GNS native | miss |", report)
            with patch.dict(os.environ, {"GITHUB_STEP_SUMMARY": str(path), "RUNNER_OS": "Linux"}):
                build_cache.summary()
            self.assertIn("| vcpkg binary | not used on this OS |", path.read_text())


if __name__ == "__main__":
    unittest.main()
