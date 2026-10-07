# Local initialization and payload safety extensions

Source: published `game-networking-sockets` 0.3.0, upstream gns-rs commit
`1d56f5a7c79889014e0b3a6106c95698fb235528` (`gns/`), by Hussein Ait-Lahcen and
James M De Broeck. The upstream package declares `MIT / Apache-2.0` licensing;
its published package and source commit contain no separate license files.
The upstream source, README, changelog and integration tests are retained.

The local extension adds `GnsGlobal::get_with_identity`, using the same lock and
singleton as `get`. Exactly one native Init receives the caller's identity.
Later identity requests validate the actual singleton and reject a mismatch,
without native reinitialization or ResetIdentity. A concurrent native unit test
counts Init calls and checks the default accessor and conflicting identity.

The payload extension adds `try_message_payload` for the shared Direct-IP/P2P
native boundary and `GnsNetworkMessage::try_payload` for checked wrapper access.
It rejects negative or unrepresentable slice lengths and nonempty null data,
and returns an empty slice for size zero before constructing a native slice.
The existing `payload` signature remains available and delegates to the same
validation. Application message limits remain in the transport adapters, so
valid owned payloads above the native default send limit remain inspectable.
Synthetic unit tests cover these representations; Direct-IP localhost tests
cover empty reliable messages on all lanes before and after authentication.

Outbound allocation also checks that the payload length fits the native `i32`
size field. Larger payloads are reclaimed with their original pointer and
`usize` length before `allocate_message` panics; the native message allocation
is released as well. The safe API and its return type remain unchanged.
Metadata-only tests cover the signed limit and wrapping lengths, including
exactly-once payload and message cleanup, without allocating multi-GiB buffers.

The root crates.io patch replaces only this high-level wrapper; sys 0.3.0 and
its bundled native GNS remain registry dependencies under the root lockfile.
Default builds still do not compile GNS. When upgrading gns-rs, remove this patch
once equivalent identity initialization and payload safety are provided upstream.

The companion `../game-networking-sockets-sys` Cargo patch adds native ICE live TURN credential updates. See its `JIGSALL_PATCH.md` for provenance and scope.
