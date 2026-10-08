"""Build CI targets once, then run the resulting test executables."""

import argparse
import json
import os
from pathlib import Path
import subprocess


ROOT = Path(__file__).resolve().parents[2]
WRAPPER_MANIFEST = ROOT / "vendor/game-networking-sockets/Cargo.toml"


def build(all_features=False):
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"],
        cwd=ROOT, encoding="utf-8",
    ))
    members = set(metadata["workspace_members"])
    command = ["cargo", "build", "--locked",
               "--bins", "--tests", "--message-format=json-render-diagnostics"]
    if all_features:
        command.extend(["--all-features", "--examples"])
    else:
        command.extend(["--features", "rendezvous"])
    # Include the excluded wrapper's initializer test in the same feature graph.
    for member in metadata["workspace_members"]:
        command.extend(["-p", member])
    command.extend(["-p", "game-networking-sockets"])
    print(" ".join(command), flush=True)
    tests = []
    identity = None
    application = None
    native = None
    commit = None
    with subprocess.Popen(command, cwd=ROOT, stdout=subprocess.PIPE,
                          encoding="utf-8") as process:
        for line in process.stdout:
            try:
                message = json.loads(line)
            except json.JSONDecodeError:
                print(line, end="", flush=True)
                continue
            if message.get("reason") == "build-script-executed" and "game-networking-sockets-sys" in message["package_id"]:
                native = message["out_dir"]
            if message.get("reason") == "build-script-executed" and "#jigsall-game@" in message["package_id"]:
                commit = dict(message["env"]).get("JIGSALL_GIT_SHA")
            if message.get("reason") != "compiler-artifact" or not message.get("executable"):
                continue
            manifest = Path(message["manifest_path"])
            if message["profile"]["test"]:
                if message["package_id"] in members:
                    tests.append(message)
                elif manifest == WRAPPER_MANIFEST and message["target"]["kind"] == ["lib"]:
                    identity = message
            elif manifest == ROOT / "Cargo.toml" and message["target"]["kind"] == ["bin"]:
                application = message
        if process.wait() != 0:
            raise subprocess.CalledProcessError(process.returncode, command)
    if not tests or identity is None or application is None or native is None or commit is None:
        raise RuntimeError("Build did not emit the workspace tests, GNS initializer, and application")
    print(f"Linked application: {application['executable']}", flush=True)
    # Packaging smoke checks reuse this already-built executable and its exact
    # native dependency tree. Only the tag workflow makes release distributions.
    dist = ROOT / "dist"
    dist.mkdir(exist_ok=True)
    (dist / "build.json").write_text(json.dumps({
        "binary": application["executable"], "native": native,
        "commit": commit,
        "features": "gns,rendezvous,tracy,chrome" if all_features else "gns,rendezvous",
    }), encoding="utf-8")
    return sorted(tests, key=lambda test: (test["manifest_path"], test["target"]["name"])), identity


def run_test(artifact, arguments):
    executable = Path(artifact["executable"])
    package_root = Path(artifact["manifest_path"]).parent
    environment = os.environ.copy()
    # Match Cargo's package working directory and dynamic library lookup paths.
    environment["CARGO_MANIFEST_DIR"] = str(package_root)
    library_path = "PATH" if os.name == "nt" else "LD_LIBRARY_PATH"
    environment[library_path] = os.pathsep.join([
        str(executable.parent), str(executable.parent.parent),
        *([environment[library_path]] if environment.get(library_path) else []),
    ])
    command = [str(executable), *arguments]
    print(" ".join(command), flush=True)
    subprocess.run(command, cwd=package_root, env=environment, check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--all-features", action="store_true",
                        help="build all features and examples (Linux CI)")
    options = parser.parse_args()
    configuration = "all features" if options.all_features else "rendezvous / GNS"
    print(f"::group::Application and test build ({configuration})", flush=True)
    tests, identity = build(options.all_features)
    print("::endgroup::", flush=True)
    print("::group::Runtime and UI tests", flush=True)
    for test in tests:
        run_test(test, ["--skip", "gns_localhost"])
    print("::endgroup::", flush=True)
    # Native GNS has a process-wide singleton: isolate and serialize localhost tests.
    print("::group::GNS localhost integration tests", flush=True)
    for test in tests:
        run_test(test, ["gns_localhost", "--nocapture", "--test-threads=1"])
    print("::endgroup::", flush=True)
    print("::group::GNS wrapper result/payload safety and single-shot identity initialization", flush=True)
    run_test(identity, ["result_tests", "--nocapture"])
    run_test(identity, ["payload_tests", "--nocapture"])
    run_test(identity, ["identity_initialization", "--nocapture"])
    print("::endgroup::", flush=True)


if __name__ == "__main__":
    main()
