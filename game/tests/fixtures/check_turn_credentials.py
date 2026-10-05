"""Exercise allocation credential binding against the local UDP TURN fixture."""
import hashlib
import hmac
import os
import socket
import struct
import sys

COOKIE = 0x2112A442
host, port = sys.argv[1].split(":")
target = (host, int(port))
sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
sock.bind((host, 0))
sock.settimeout(2)


def attr(kind, data):
    return struct.pack("!HH", kind, len(data)) + data + b"\0" * ((-len(data)) % 4)


def request(kind, attrs, key=None):
    tx = os.urandom(12)
    body = b"".join(attrs)
    header = struct.pack("!HHI", kind, len(body) + (24 if key else 0), COOKIE) + tx
    if key:
        body += attr(8, hmac.new(key, header + body, hashlib.sha1).digest())
    sock.sendto(header + body, target)
    data, source = sock.recvfrom(65535)
    assert source == target and data[8:20] == tx
    code = struct.unpack("!H", data[:2])[0]
    values = {}
    offset = 20
    while offset + 4 <= len(data):
        k, length = struct.unpack("!HH", data[offset:offset + 4])
        values[k] = data[offset + 4:offset + 4 + length]
        if k == 8:
            assert key is not None
            signed = data[:2] + struct.pack("!H", offset + 4) + data[4:20] + data[20:offset]
            assert hmac.compare_digest(values[k], hmac.new(key, signed, hashlib.sha1).digest())
        offset += 4 + ((length + 3) // 4) * 4
    if key:
        assert 8 in values, "authenticated response must include MESSAGE-INTEGRITY"
    return code, values


transport = attr(0x19, bytes([17, 0, 0, 0]))
code, challenge = request(3, [transport])
assert code == 0x113 and challenge[9][2:4] == bytes([4, 1])
realm, nonce = challenge[0x14], challenge[0x15]


def authenticated(kind, version, extra):
    user = ("user-" + version).encode()
    password = ("password-" + version).encode()
    key = hashlib.md5(user + b":" + realm + b":" + password).digest()
    return request(kind, [attr(6, user), attr(0x14, realm), attr(0x15, nonce)] + extra, key)


code, values = authenticated(3, "A", [transport])
assert code == 0x103 and 0x16 in values
lifetime = attr(0x0d, struct.pack("!I", 4))
assert authenticated(4, "A", [lifetime])[0] == 0x104
peer = struct.pack("!BBH", 0, 1, int(port) ^ (COOKIE >> 16)) + bytes(
    a ^ b for a, b in zip(socket.inet_aton(host), struct.pack("!I", COOKIE)))
for kind, extra in [(4, [lifetime]), (8, [attr(0x12, peer)])]:
    code, values = authenticated(kind, "B", extra)
    assert code == (kind | 0x110) and values[9][2:4] == bytes([4, 41]), "credential change must return 441"
assert authenticated(4, "A", [lifetime])[0] == 0x104
assert authenticated(8, "A", [attr(0x12, peer)])[0] == 0x108
sock.close()
print("RFC 8656 allocation credential binding: passed")
