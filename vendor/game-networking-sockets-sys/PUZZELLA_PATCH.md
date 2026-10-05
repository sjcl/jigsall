# Puzzella native ICE patch

Base: crates.io `game-networking-sockets-sys` 0.3.0, archive SHA256
`8b9d11200371f3b60115e9b4a9078cb649b4c9fbc46fa8fa356f725974b28172`.
The vendored source preserves the upstream license and dependency versions.
The root Cargo patch and lockfile select this source for the existing wrapper.
No registry cache or system GNS installation is modified.

Modified upstream files:

- `include/steam/steamnetworkingsockets_flat.h`: `Puzzella_UpdateTURN` C entrypoint.
- `src/steamnetworkingsockets/clientlib/steamnetworkingsockets_p2p_ice.h`: a
  virtual update hook, default unsupported for non-native ICE.
- `src/steamnetworkingsockets/clientlib/steamnetworkingsockets_ice_client.h/.cpp`:
  copy live credentials under native locks; regenerate realm authentication key;
  preserve in-flight packet/key; force next Refresh with the installed value;
  handle bounded 401/438 challenges; require success before advancing refresh or
  permission state; retain existing refresh-error/timeout reallocation; renew
  permissions during long sessions; remove username logging from auth challenges.
- `build.rs`: search CMake's installed `lib` directory. Optimized Rust dev profiles
  use MSVC `RelWithDebInfo`, whereas upstream assumes `Debug` for all dev builds.
- `Cargo.toml`: standalone workspace declaration for the local Cargo patch.

Server addresses are fixed for an existing ICE session. Updates match the original
configured host:port rather than re-resolving DNS, retaining the association with
all resolved addresses and their live allocations. An update with no matching
initialized entry returns false. Direct sessions with no TURN server reject late
installation; rotation requires the initial address set.
Future connections on a TURN-enabled control session inherit its latest credentials.
Changing relay topology or supporting TCP/TLS/WebRTC ICE is separate work.

Tests and protocol documentation: `docs/RENDEZVOUS_V1.md`.
