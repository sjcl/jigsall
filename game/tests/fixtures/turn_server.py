"""Local UDP TURN test double. No external service, secrets or Python dependencies.
Verifies RFC 5389 long-term MESSAGE-INTEGRITY and relays real UDP packets.
Binds each allocation to its initial authentication key (RFC 8656 sections 5/6).
"""
import hashlib, hmac, json, os, queue, selectors, socket, struct, sys, threading, time
COOKIE = 0x2112A442
REALM = b"puzzella-test"
KEYS = {b"user-" + v: b"password-" + v for v in (b"A", b"B", b"C")}
relay_pairs_only = "--relay-pairs-only" in sys.argv[1:]
sel = selectors.DefaultSelector()
server = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
server.bind(("0.0.0.0", 0))
probe = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
probe.connect(("192.0.2.1", 9))
LAN = probe.getsockname()[0]
probe.close()
sel.register(server, selectors.EVENT_READ, None)
allocations = {}
allocation_auth = {}
invalid_clients = set()
commands = queue.Queue()
nonce = b"nonce-1"
reject_a = False
stale = False
hold = False
fail = False
delayed = []
stats = {"allocate_a": 0, "allocate_b": 0, "refresh_a": 0, "refresh_b": 0,
         "permission_a": 0, "permission_b": 0, "allocate_c": 0, "refresh_c": 0,
         "permission_c": 0, "allocate_requests": 0, "wrong_credentials": 0,
         "stale": 0, "refresh_fail": 0, "bad_auth": 0, "relayed": 0, "held": 0, "wrong_allocations": 0}
def stdin():
    for line in sys.stdin:
        commands.put(line.strip())
threading.Thread(target=stdin, daemon=True).start()
def attr(kind, data):
    return struct.pack("!HH", kind, len(data)) + data + b"\0" * ((-len(data)) % 4)
def packet(kind, tx, attrs, key=None):
    body = b"".join(attrs)
    header = struct.pack("!HHI", kind, len(body) + (24 if key else 0), COOKIE) + tx
    if key:
        body += attr(8, hmac.new(key, header + body, hashlib.sha1).digest())
    return header + body
def xor_addr(address):
    return struct.pack("!BBH", 0, 1, address[1] ^ (COOKIE >> 16)) + bytes(a ^ b for a, b in zip(socket.inet_aton(address[0]), struct.pack("!I", COOKIE)))
def read_addr(data):
    port = struct.unpack("!H", data[2:4])[0] ^ (COOKIE >> 16)
    ip = socket.inet_ntoa(bytes(a ^ b for a, b in zip(data[4:8], struct.pack("!I", COOKIE))))
    return ip, port
def response_error(kind, tx, client, code, key=None):
    return packet(kind | 0x110, tx, [attr(9, bytes([0, 0, code // 100, code % 100])), attr(0x14, REALM), attr(0x15, nonce)], key)
print(LAN + ":" + str(server.getsockname()[1]), flush=True)
while True:
    try:
        command = commands.get_nowait()
        if command == "stop": break
        if command == "stats": print("STATS:" + json.dumps(stats), flush=True)
        if command == "stale": stale = True
        if command == "hold": hold = True
        if command == "fail": fail = True
        if command == "expire_a": reject_a = True
    except queue.Empty:
        pass
    now = time.monotonic()
    for due, data, client in delayed[:]:
        if due <= now:
            server.sendto(data, client)
            delayed.remove((due, data, client))
    for event, _ in sel.select(0.01):
        sock = event.fileobj
        try:
            data, source = sock.recvfrom(65535)
        except ConnectionResetError:
            # Windows reports an ICMP reply to an already closed peer on recv.
            continue
        if event.data is not None:
            # Forced-relay integration requires both endpoints to use allocations.
            # Otherwise native ICE can accept a one-sided peer-reflexive route.
            if relay_pairs_only and source not in {relay.getsockname() for relay in allocations.values()}:
                continue
            client = event.data
            server.sendto(packet(0x17, os.urandom(12), [attr(0x12, xor_addr(source)), attr(0x13, data)]), client)
            stats["relayed"] += 1
            continue
        if len(data) < 20 or struct.unpack("!I", data[4:8])[0] != COOKIE: continue
        kind = struct.unpack("!H", data[:2])[0]
        tx = data[8:20]
        attrs = {}
        offsets = {}
        offset = 20
        while offset + 4 <= len(data):
            k, length = struct.unpack("!HH", data[offset:offset+4])
            attrs[k] = data[offset+4:offset+4+length]
            offsets[k] = offset
            offset += 4 + ((length + 3) // 4) * 4
        if kind == 0x16:
            if source in allocations and 0x12 in attrs and 0x13 in attrs:
                allocations[source].sendto(attrs[0x13], read_addr(attrs[0x12]))
            continue
        if kind == 3: stats["allocate_requests"] += 1
        user = attrs.get(6)
        key = hashlib.md5(user + b":" + REALM + b":" + KEYS[user]).digest() if user in KEYS else None
        authenticated = False
        if key and 8 in attrs:
            offset = offsets[8]
            header = data[:2] + struct.pack("!H", offset + 24 - 20) + data[4:20]
            authenticated = hmac.compare_digest(attrs[8], hmac.new(key, header + data[20:offset], hashlib.sha1).digest())
        if not authenticated or (reject_a and user == b"user-A"):
            stats["bad_auth"] += bool(user)
            if user and 8 in attrs and not authenticated: invalid_clients.add(source)
            server.sendto(response_error(kind, tx, source, 401, key), source)
            continue
        if kind != 3 and source in allocation_auth and allocation_auth[source] != (user, key):
            stats["wrong_credentials"] += 1
            server.sendto(response_error(kind, tx, source, 441, key), source)
            continue
        if kind == 4 and stale:
            nonce = b"nonce-2"
            stale = False
            stats["stale"] += 1
            server.sendto(response_error(kind, tx, source, 438, key), source)
            continue
        if attrs.get(0x15) != nonce:
            stats["stale"] += 1
            server.sendto(response_error(kind, tx, source, 438, key), source)
            continue
        if kind == 4 and fail:
            fail = False
            stats["refresh_fail"] += 1
            relay = allocations.pop(source, None)
            allocation_auth.pop(source, None)
            if relay is not None:
                sel.unregister(relay)
                relay.close()
            server.sendto(response_error(kind, tx, source, 437, key), source)
            continue
        if kind in (4, 8) and source not in allocations:
            server.sendto(response_error(kind, tx, source, 437, key), source)
            continue
        response_attrs = []
        if kind == 3:
            stats["wrong_allocations"] += source in invalid_clients
            if source not in allocations:
                relay = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
                relay.bind((LAN, 0))
                allocations[source] = relay
                allocation_auth[source] = (user, key)
                sel.register(relay, selectors.EVENT_READ, source)
            stats["allocate_" + user[-1:].decode().lower()] += 1
            response_attrs = [attr(0x16, xor_addr(allocations[source].getsockname())), attr(0x20, xor_addr(source)), attr(0x0d, struct.pack("!I", 4))]
        elif kind == 4:
            stats["refresh_" + user[-1:].decode().lower()] += 1
            response_attrs = [attr(0x0d, struct.pack("!I", 4))]
        elif kind == 8:
            stats["permission_" + user[-1:].decode().lower()] += 1
        else: continue
        response = packet(kind | 0x100, tx, response_attrs, key)
        if kind == 4 and hold:
            hold = False
            stats["held"] += 1
            delayed.append((time.monotonic() + 0.75, response, source))
        else: server.sendto(response, source)
for relay in allocations.values(): relay.close()
server.close()
