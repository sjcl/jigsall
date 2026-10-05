"""Local cross-repository integration harness. No production API or secrets.
Usage: python run_turn_smoke.py RENDEZVOUS_EXAMPLE_BINARY GAME_TEST_BINARY
"""
import json, os, pathlib, subprocess, sys
here = pathlib.Path(__file__).parent
fixture = subprocess.Popen([sys.executable, str(here / "turn_server.py"), "--relay-pairs-only"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
server = None
def stats():
    fixture.stdin.write("stats\n"); fixture.stdin.flush()
    line = fixture.stdout.readline()
    assert line.startswith("STATS:")
    return json.loads(line[len("STATS:"):])
try:
    address = fixture.stdout.readline().strip()
    assert address, "local TURN fixture requires a private IPv4 interface"
    for mode in ["available", "unavailable", "recovering"]:
        for test in ["network::gns::p2p::tests::rendezvous_smoke::gns_localhost_real_rendezvous_native_ice_password_secure_lanes",
                     "network::runtime::rendezvous::tests::smoke::gns_localhost_real_rendezvous_runtime_ready_command_roundtrip"]:
            # Restart so each recovering case begins before mock API recovery.
            before = stats()
            server = subprocess.Popen([sys.argv[1], address, mode], stdout=subprocess.PIPE, text=True)
            url = server.stdout.readline().strip()
            assert url.startswith("ws://127.0.0.1:"), "fixture must stay local"
            env = os.environ.copy()
            env["JIGSALL_RENDEZVOUS_SMOKE_URL"] = url
            env.pop("JIGSALL_TURN_TEST_ONLY", None)
            env.pop("JIGSALL_TURN_INITIAL_UNAVAILABLE", None)
            if mode == "available": env["JIGSALL_TURN_TEST_ONLY"] = "1"
            else: env["JIGSALL_TURN_INITIAL_UNAVAILABLE"] = "1"
            result = subprocess.run([sys.argv[2], "--exact", test, "--ignored", "--nocapture", "--test-threads=1"], env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, timeout=70)
            assert all(secret not in result.stdout for secret in ["user-A", "user-B", "password-A", "password-B"]), "TURN credentials appeared in test logs"
            print(result.stdout, end="")
            assert result.returncode == 0, "local integration test failed"
            after = stats()
            if mode == "available":
                assert after["relayed"] > before["relayed"] and after["refresh_a"] > before["refresh_a"], after
                assert after["allocate_b"] == before["allocate_b"] and after["refresh_b"] == before["refresh_b"], "existing connection replaced allocation credentials"
                assert after["wrong_credentials"] == before["wrong_credentials"], after
            else:
                assert after["allocate_a"] == before["allocate_a"] and after["allocate_b"] == before["allocate_b"], "direct-only session gathered late TURN"
            server.terminate(); server.wait(timeout=5); server = None
    print("Local TURN fixed allocation credentials + SPAKE2 + Sync/Ready; control loss survival; initial unavailable sessions remain direct-only after provider recovery: passed")
finally:
    if server is not None: server.terminate(); server.wait(timeout=5)
    fixture.terminate(); fixture.wait(timeout=5)
