# Jigsall native ICE, payload release and result binding patches

Base: crates.io `game-networking-sockets-sys` 0.3.0, archive SHA256
`8b9d11200371f3b60115e9b4a9078cb649b4c9fbc46fa8fa356f725974b28172`.
The vendored source preserves the upstream license and dependency versions.
The root Cargo patch and lockfile select this source for the existing wrapper.
No registry cache or system GNS installation is modified.

Modified upstream files:

- `src/steamnetworkingsockets/clientlib/steamnetworkingsockets_snp.h/.cpp`
  and `steamnetworkingsockets_connections.cpp`: retain the reliable header size
  in a private message field and remove it from `m_cbSize` only at final release,
  after queue accounting, before invoking the application free callback. Rust
  `Payload::from_raw` therefore receives its original allocation length for
  empty and nonempty buffers. Unsent, failed, received and local-pipe messages
  retain a zero header adjustment. Public message ABI, payload pointers and
  user data are unchanged. Direct-IP localhost tests cover empty `Vec` sends and
  exact-once callback lengths across reliable header size boundaries.

- `src/steamnetworkingsockets/clientlib/steamnetworkingsockets_p2p_ice.cpp`:
  snapshot and lock TURN server/user/password configuration when ICE initializes,
  so later listener defaults cannot change an existing connection.
- `src/steamnetworkingsockets/clientlib/steamnetworkingsockets_ice_client.h/.cpp`:
  handle bounded 401/438 challenges using the original allocation credentials;
  require success before advancing refresh or permission state; retain existing
  refresh-error/timeout reallocation; renew permissions during long sessions;
  remove username logging from auth challenges.
- `build.rs`: search CMake's installed `lib` directory. Optimized Rust dev profiles
  use MSVC `RelWithDebInfo`, whereas upstream assumes `Debug` for all dev builds.
  Generate `EResult` as an integer newtype with associated constants, so unknown
  native result codes remain valid Rust values while preserving the native ABI.
- `Cargo.toml`: standalone workspace declaration for the local Cargo patch.

[RFC 8656 sections 5/6](https://www.rfc-editor.org/rfc/rfc8656.html#section-5) bind
allocation authentication to its initial credentials. Existing connections keep
those credentials for Refresh, CreatePermission and automatic reallocation.
Nonce refresh is separate from credential replacement. There is no active
allocation credential-update API; the former native API was removed.

New defaults apply only to listener inheritance and future outgoing connections.
The initial endpoint set must remain unchanged, and an initially direct-only
control session never gains TURN later. New defaults do not extend existing
connection credential expiry. ICE restart/migration, changed relay topology,
TCP/TLS and WebRTC ICE support are separate work.

Tests and protocol documentation: `docs/RENDEZVOUS_V1.md`.
