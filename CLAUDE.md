# CLAUDE.md

Repository guidance for Claude Code. Read [INSTRUCTION.md](INSTRUCTION.md) for the development scope and [ARCHITECTURE.md](ARCHITECTURE.md) for the investigation, current design, and remaining work.

## Build and verification

Rust 1.95+ is required. Windows is the verified build platform; see [WINDOWS_BUILD.md](WINDOWS_BUILD.md).

```sh
cargo run --locked
cargo build --locked --release
cargo fmt --check
cargo check --locked
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked
```

Use release mode for performance measurements. Tracy and Chrome tracing remain available through the `tracy` and `chrome` features. Do not claim 60fps or cross-platform runtime compatibility without measuring it.

## Current architecture

- Bevy 0.19.1 and bevy_egui 0.42. Windows pins wgpu-hal 29.0.3 for dependency compatibility.
- AppState is Menu / GameSetup / InGame / GameComplete. GameSubState is Initializing / Playing / Paused and exists only inside InGame. There is no GameScreen or legacy compatibility state.
- `gameplay.rs` owns serializable PuzzleDefinition, stable PieceId / PlayerId, immutable PuzzlePiece, mutable PieceState, and pure command / snap decisions.
- `networking.rs` provides the local ClientCommand boundary. There is no active transport, Renet integration, host / join implementation, or port UI.
- Input emits Grab / Move / Release commands. Game logic validates ownership and coordinates, then projects state into rendering and collision caches. Input must not directly change gameplay Transform values.
- PieceDataStore holds all canonical piece records, including pieces without individual Entities. Temporary Entities and batch meshes are local presentation. Progress and snap must count the canonical store.
- `interaction.rs` owns the Idle / Dragging / BoxSelecting gesture; `InputState` only samples pointer validity and camera state.
- Interaction runs in PostUpdate after the current egui pass and camera changes, before transform propagation. MainCamera is a root entity; zoom scales XY only.
- Selection is stored as sets of PieceId. R-tree and triangle tests perform picking; Bevy pointer picking is not the gameplay input path.

## Preserve these systems

- `jigsaw_shapes.rs`: puzzle-paths, SVG geometry, lyon tessellation, original UVs and stroke meshes.
- `puzzle.rs`: seeded concentric-circle and fallback placement / shuffle.
- `systems/puzzle_generation.rs`: background CPU generation, channels, then main-thread asset creation at ten pieces per frame. Workers never access World or GPU resources.
- `systems/input_camera.rs`: pan, zoom, adaptive camera framing and edge scrolling.
- `interaction.rs` / `piece_geometry.rs`: single / Ctrl / box selection, relative multi-drag offsets, indexed triangle collision and final-release coordinates.
- `systems/piece_interaction.rs`: input adapters, cached child outlines and one selection rectangle updated by Transform.
- `systems/batching.rs`: contiguous Z ranges split around extracted pieces, preserving order, UVs and image transparency.
- `asset_reader.rs` and `systems/image_loading.rs`: external file registry and worker image decode. File dialog itself is synchronous.
- R-tree collision cache, stroke cache, batch extraction / return, change detection, F12 performance monitoring, Tracy / Chrome tracing.

Keep one original image texture and share the normal material. Retain original meshes and UVs when rebuilding batches. Generation is versioned and seeded; cross-platform bit equality still needs validation.

Future multiplayer must authenticate commands and validate session / sequence / movement at the host. Clients must never dictate `placed`. Design snapshot and authoritative event types before choosing Lightyear or Quinn.
