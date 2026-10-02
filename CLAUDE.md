# CLAUDE.md

Repository guidance for Claude Code. Read [INSTRUCTION.md](docs/INSTRUCTION.md) for the development scope, [GPU_PICKING.md](docs/GPU_PICKING.md) for the current GPU picking implementation and checks, and [ARCHITECTURE.md](docs/ARCHITECTURE.md) for the gameplay architecture.

## Build and verification

Rust 1.95+ is required. Windows is the verified build platform; see [WINDOWS_BUILD.md](docs/WINDOWS_BUILD.md).

```sh
cargo run --locked
cargo build --locked --release
cargo fmt --all --check
cargo check --workspace --locked
cargo clippy --workspace --locked --all-targets --all-features -- -D warnings
cargo test --workspace --locked
```

Use release mode for performance measurements. Tracy and Chrome tracing remain available through the `tracy` and `chrome` features. Do not claim 60fps or cross-platform runtime compatibility without measuring it.

## Current architecture

- Bevy 0.19.1 and bevy_egui 0.42. Windows pins wgpu-hal 29.0.3 for dependency compatibility.
- AppState is Menu / GameSetup / InGame / GameComplete. GameSubState is Initializing / Playing / Paused and exists only inside InGame. GameCompleteSubState is Summary / Viewing / Paused and preserves the completed session. There is no GameScreen or legacy compatibility state.
- The Cargo workspace contains the thin `puzzella` binary and `puzzella-core`, `puzzella-puzzle`, `puzzella-game`, `puzzella-ui` libraries. Dependency versions and package metadata are inherited from the root manifest; all packages share one lockfile. See docs/ARCHITECTURE.md for ownership and dependency direction.
- `core/src/gameplay.rs` owns serializable PuzzleDefinition, stable PieceId / PlayerId, immutable PuzzlePiece, mutable PieceState, and pure command / snap decisions. It depends only on ECS markers, math and serde, without rendering or UI.
- `core/src/commands.rs` provides the local ClientCommand boundary. There is no active transport, Renet integration, host / join implementation, or port UI.
- `puzzella-puzzle` owns CPU shape / placement / mesh generation. `puzzella-game` owns the Bevy lifecycle, workers and presentation. `puzzella-ui` owns egui screens and GameUiPlugin. Resource definitions live in `game/src/resources/`; GPU selection is split into API, coordinates and render modules under `selection/`.
- Input emits Grab / Move / Release commands. Game logic validates ownership and coordinates, then projects state into rendering and collision caches. Input must not directly change gameplay Transform values.
- PieceDataStore holds all canonical piece records, including pieces without individual Entities. Temporary Entities and batch meshes are local presentation. Progress and snap must count the canonical store.
- `interaction.rs` owns the Idle / Dragging / BoxSelecting gesture; `InputState` only samples pointer validity and camera state.
- Interaction runs in PostUpdate after the current egui pass and camera changes, before transform propagation. MainCamera is a root entity; zoom scales XY only.
- Selection is stored as sets of PieceId. PuzzleSelectionPlugin performs GPU picking with shared rendered buffers. R-tree and triangle tools exist only under cpu-picking-debug or tests.

## Preserve these systems

- `puzzle/src/shapes.rs` / `shape_data.rs`: puzzle-paths, SVG geometry, lyon tessellation, original UVs and stroke meshes, shared PieceShape.
- `puzzle/src/placement.rs`: seeded concentric-circle and fallback placement / shuffle.
- `systems/puzzle_generation.rs`: background CPU generation, channels, then main-thread asset creation at ten pieces per frame. Workers never access World or GPU resources.
- `systems/input_camera.rs`: pan, zoom, adaptive camera framing and edge scrolling.
- `interaction.rs` / `piece_geometry.rs`: single / Ctrl / box selection, relative multi-drag offsets, asynchronous GPU results and final-release coordinates.
- `systems/piece_interaction.rs`: input adapters, cached child outlines and one selection rectangle updated by Transform.
- `systems/batching.rs`: contiguous Z ranges split around extracted pieces, preserving order, UVs and image transparency.
- `asset_reader.rs` and `systems/image_loading.rs`: external file registry and worker image decode. File dialog itself is synchronous.
- GPU picking, stroke cache, batch extraction / return, change detection, F3 performance overlay, Tracy / Chrome tracing.

Keep one original image texture and share the normal material. Retain original meshes and UVs when rebuilding batches. Generation is versioned and seeded; cross-platform bit equality still needs validation.

Future multiplayer must authenticate commands and validate session / sequence / movement at the host. Clients must never dictate `placed`. Design snapshot and authoritative event types before choosing Lightyear or Quinn.
