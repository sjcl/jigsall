# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build and Development Commands

**Build the project:**
```bash
cargo build --release
```

**Run the application:**
```bash
cargo run
```

**Check for compilation errors:**
```bash
cargo check
```

**Clean build artifacts:**
```bash
cargo clean
```

**Development notes:**
- The project is designed for Windows environments primarily (see WINDOWS_BUILD.md)
- WSL environments have known issues with winit library compilation
- Use release mode for better performance when testing

## Architecture Overview

Puzzella is a multiplayer jigsaw puzzle game built with Bevy 0.16.1. The architecture follows Bevy's ECS (Entity Component System) pattern with modular organization.

### Core Architecture Components

**Main Application Structure:**
- `main.rs`: Application entry point using Bevy App with DefaultPlugins, bevy_egui, and custom GamePlugin
- UI systems run on `EguiPrimaryContextPass` schedule for proper egui integration

**Key Modules:**
- `game.rs`: Core game plugin managing startup and update systems
- `components.rs`: ECS components including PuzzlePiece, PickablePiece (for Bevy's picking system), and UI markers
- `resources.rs`: Global game state including GameState, PuzzleConfig, PuzzleImage, and InputState
- `systems.rs`: Game logic systems for input handling, piece dragging, placement checking, and camera controls
- `puzzle.rs`: Puzzle generation, image loading with asset path conversion, and piece creation
- `jigsaw_shapes.rs`: Complex jigsaw piece shape generation using lyon tessellation and puzzle-paths library
- `ui.rs`: egui-based interface for menus, game setup, and in-game UI

### Input and Interaction System

The project uses **Bevy's built-in picking system** (introduced in Bevy 0.15+) for piece selection and dragging:
- `PickablePiece` component enables pieces to respond to pointer events
- Observer pattern with `DragStart`, `Drag`, and `DragEnd` events
- Automatic hit detection with alpha transparency support
- Legacy `Draggable` component maintained for compatibility during transition

### Asset Management

**Dynamic Image Loading:**
- File dialog integration using `rfd` crate for runtime image selection
- Automatic copying of images from absolute paths to `assets/` folder for Bevy compatibility
- Asset path conversion handling for cross-platform support

**Jigsaw Shape Generation:**
- Uses `puzzle-paths` crate for generating realistic jigsaw piece templates
- `lyon` tessellation for converting SVG paths to Bevy meshes
- Shape caching system to avoid regenerating identical pieces
- Bounding box calculation for accurate hit detection

### State Management

**Game Flow:**
- `GameScreen` enum manages application state (Menu → HostSetup/JoinGame → InGame → GameComplete)
- `PuzzleConfig` handles grid size, piece size, snap distance, and image path
- `InputState` tracks mouse position, selected pieces, and camera controls

**Camera System:**
- 2D camera with zoom and pan controls
- Auto-zoom adjustment based on puzzle image size
- Right-click drag for camera movement
- Mouse wheel zoom with configurable limits

### Networking (Currently Disabled)

The networking module is temporarily disabled due to dependency conflicts:
- Originally used `renet` for multiplayer functionality
- `bevy_renet` integration for Bevy compatibility
- Network state management through `NetworkInfo` resource

### UI System Integration

**egui Integration:**
- Custom UI systems run on `EguiPrimaryContextPass` schedule
- Context error handling with proper fallback patterns
- Japanese font support for Windows environments
- File dialog integration for image selection

### Development Context

**Version Compatibility:**
- Project upgraded from Bevy 0.14.2 to 0.16.1
- Networking temporarily disabled due to version conflicts between bevy_renet and newer Bevy versions
- Uses `bevy_picking` feature explicitly in Cargo.toml

**Known Implementation Details:**
- Manual asset copying workaround for dynamic file loading (considered and rejected bevy_asset_loader due to incompatibility with file dialog workflow)
- Transition from manual hit detection to Bevy's picking system (old `handle_piece_dragging` disabled)
- Z-order management for piece layering during drag operations
- Snap-to-grid functionality with configurable tolerance

**Platform Considerations:**
- Windows-first development approach
- File dialog behavior differs between platforms
- Asset path handling for absolute vs relative paths