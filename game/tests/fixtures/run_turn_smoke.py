"""Manual cross-repository integration harness, using only local sockets.
Usage: python run_turn_smoke.py RENDEZVOUS_EXAMPLE_BINARY GAME_TEST_BINARY
Build the example turn_fixture_server and the game lib tests before running.
"""
import json, os, pathlib, subprocess, sys
here = pathlib.Path(__file__).parent
fixture = subprocess.Popen([sys.executable, str(here / "turn_server.py")], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
server = None
try:
    address = fixture.stdout.readline().strip()
    assert address, "local TURN fixture requires a private IPv4 interface"
    for unavailable in [False, True]:
        command = [sys.argv[1], address] + (["unavailable"] if unavailable else [])
        server = subprocess.Popen(command, stdout=subprocess.PIPE, text=True)
        url = server.stdout.readline().strip()
        assert url.startswith("ws://127.0.0.1:"), "fixture must stay local"
        env = os.environ.copy()
        env["JIGSALL_RENDEZVOUS_SMOKE_URL"] = url
        if unavailable: env.pop("JIGSALL_TURN_TEST_ONLY", None)
        else: env["JIGSALL_TURN_TEST_ONLY"] = "1"
        for test in ["network::gns::p2p::tests::rendezvous_smoke::gns_localhost_real_rendezvous_native_ice_password_secure_lanes",
                     "network::runtime::rendezvous::tests::smoke::gns_localhost_real_rendezvous_runtime_ready_command_roundtrip"]:
            result = subprocess.run([sys.argv[2], "--exact", test, "--ignored", "--nocapture", "--test-threads=1"], env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, timeout=70)
            assert all(secret not in result.stdout for secret in ["user-A", "user-B", "password-A", "password-B"]), "TURN credentials appeared in test logs"
            print(result.stdout, end="")
            assert result.returncode == 0, "local integration test failed"
        server.terminate(); server.wait(timeout=5); server = None
    fixture.stdin.write("stats\n"); fixture.stdin.flush()
    line = fixture.stdout.readline()
    assert line.startswith("STATS:")
    stats = json.loads(line[len("STATS:"):])
    assert stats["relayed"] > 0 and stats["refresh_b"] > 0, stats
    print("Local TURN + live WSS credentials + SPAKE2 + Sync/Ready; WSS loss survival; unavailable provider direct ICE: passed")
finally:
    if server is not None: server.terminate(); server.wait(timeout=5)
    fixture.terminate(); fixture.wait(timeout=5)
