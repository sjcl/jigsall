# Local initialization extension

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

The root crates.io patch replaces only this high-level wrapper; sys 0.3.0 and
its bundled native GNS remain registry dependencies under the root lockfile.
Default builds still do not compile GNS. When upgrading gns-rs, remove this patch
once an equivalent identity-aware initializer is provided upstream.
