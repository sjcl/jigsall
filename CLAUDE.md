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

Puzzella is a multiplayer jigsaw puzzle game built with Bevy 0.16.1. The architecture follows Bevy's ECS (Entity Component System) pattern with modular organization and performance optimization.

### Core Architecture Components

**Main Application Structure:**
- `main.rs`: Application entry point using Bevy App with DefaultPlugins, bevy_egui, and custom GamePlugin
- UI systems run on `EguiPrimaryContextPass` schedule for proper egui integration

**Key Modules:**
- `game.rs`: Core game plugin managing startup and update systems with dual state management
- `components.rs`: ECS components including PuzzlePiece, PickablePiece (for Bevy's picking system), SelectedPiece, and UI markers
- `resources.rs`: Global game state including AppState, GameSubState, PuzzleConfig, PuzzleImage, InputState, and performance monitoring
- `puzzle.rs`: Puzzle generation, progressive piece spawning, and asset management
- `jigsaw_shapes.rs`: Complex jigsaw piece shape generation using lyon tessellation and puzzle-paths library
- `asset_reader.rs`: External file registry and thread-based image loading system

**Systems Organization (Modular):**
- `systems/game_logic.rs`: Piece placement, snapping logic, and game state management with event-driven optimization
- `systems/piece_interaction.rs`: Advanced multi-selection, box selection, dragging, and highlighting systems
- `systems/puzzle_generation.rs`: Progressive puzzle piece generation and background spawning
- `systems/image_loading.rs`: Thread-based image loading with crossbeam channels
- `systems/input_camera.rs`: Camera controls, zoom, pan, and edge scrolling
- `systems/performance.rs`: Performance monitoring, profiling, and debug systems

**UI Organization (Modular):**
- `ui/menu.rs`: Main menu interface
- `ui/game_setup.rs`: Game configuration, image selection, and puzzle setup
- `ui/game_play.rs`: In-game UI elements and controls
- `ui/overlays.rs`: HUD overlays, progress bars, and status displays
- `ui/common.rs`: Shared UI utilities and components

### Input and Interaction System

The project uses a **hybrid interaction system** combining Bevy's picking system with advanced multi-selection:

**Piece Interaction:**
- `PickablePiece` component enables pieces to respond to pointer events
- Automatic hit detection with alpha transparency support
- Event-driven piece placement with `PieceMoveCompleted` and `PiecePlacedEvent`
- Legacy `Draggable` component maintained for compatibility during transition

**Advanced Selection System:**
- **Multi-Selection Modes**: Single, BoxSelection, MultiDrag via `SelectionMode` enum
- **Box Selection**: Drag-to-select multiple pieces with visual selection box rendering
- **Multi-Drag**: Drag multiple selected pieces simultaneously while maintaining relative positions
- **Selection Components**: `SelectedPiece`, `SelectionPreview`, `SelectionBox` for different selection states
- **Smart Deselection**: Pieces automatically deselected when placed in correct positions

**Performance Optimizations:**
- `PieceSelectionCache` for fast piece lookup and boundary calculations
- Cached piece positions and bounds to avoid repeated queries
- Event-driven updates to minimize per-frame processing

### Asset Management

**Thread-Based Image Loading:**
- File dialog integration using `rfd` crate for runtime image selection
- **Thread-based loading** with `std::thread::spawn` for non-blocking UI
- **Crossbeam channels** for thread-safe communication between worker threads and main thread
- `ExternalFileRegistry` for managing external file paths and virtual keys
- **ImageLoadChannels** resource for coordinating image loading results
- Automatic file format detection and RGBA conversion

**Jigsaw Shape Generation:**
- Uses `puzzle-paths` crate for generating realistic jigsaw piece templates
- `lyon` tessellation for converting SVG paths to Bevy meshes
- **Progressive generation** system for large puzzles to maintain responsiveness
- Shape caching system (`StrokeMeshCache`) to avoid regenerating identical pieces
- Bounding box calculation for accurate hit detection and selection
- **Background spawning** with configurable pieces-per-frame limits

### State Management

**Dual State System:**
- **`AppState`**: Primary application flow (Loading → Menu → GameSetup → InGame → GameComplete)
- **`GameSubState`**: In-game sub-states (Initializing → Playing → Paused → Complete)
- State transitions managed through Bevy's state system with proper Enter/Exit handlers
- Legacy `GameScreen` enum maintained for backward compatibility during transition

**Configuration Management:**
- `PuzzleConfig`: Grid size, piece modes (TargetCount, ManualGrid, SquarePieces), snap distance, image path
- **Multi-mode piece sizing**: Target piece count, manual grid, or target piece size
- Dynamic grid calculation based on image aspect ratio and target parameters

**Input and Selection State:**
- `InputState`: Comprehensive tracking of mouse position, camera state, and selection
- **Multi-selection support**: Selected pieces tracking with HashSet for fast lookup
- **Drag state management**: Multi-drag offsets, selection rectangles, camera dragging
- **Edge scrolling**: Cursor position tracking for automatic camera movement

**Camera System:**
- 2D camera with zoom and pan controls
- Auto-zoom adjustment based on puzzle image size
- Right-click drag for camera movement
- Mouse wheel zoom with configurable limits
- **Edge scrolling**: Automatic camera movement when dragging pieces near screen edges

### Performance Monitoring System

**Performance Profiling:**
- `PerformanceMonitor` resource with configurable debug levels (Off, Low, Medium, High)
- **System timing**: Individual system performance tracking with min/max/average measurements
- **Frame time monitoring**: FPS calculation and frame duration tracking
- **Report intervals**: Configurable performance reporting every N seconds

**Debug and Profiling Tools:**
- **Macro-based timing**: `time_scope!` macro for easy performance measurement
- **Tracy integration**: Support for Tracy profiler with `tracy` feature flag
- **Chrome tracing**: Browser-based performance analysis with `chrome` feature flag
- **F12 toggle**: Runtime performance debug level cycling
- **Performance statistics**: Detailed system call counts and duration tracking

**Optimization Systems:**
- **Event-driven architecture**: Minimize per-frame processing with event systems
- **Caching strategies**: Multiple cache systems for pieces, selections, and positions
- **Progressive loading**: Background piece generation to maintain 60fps
- **Smart invalidation**: Cache refresh only when necessary

### Networking (Currently Disabled)

The networking module is temporarily disabled due to dependency conflicts:
- Originally used `renet` for multiplayer functionality
- `bevy_renet` integration for Bevy compatibility
- Network state management through `NetworkInfo` resource

### UI System Integration

**Modular egui Architecture:**
- **Modular UI structure**: Separated into menu, game_setup, game_play, overlays modules
- Custom UI systems run on `EguiPrimaryContextPass` schedule
- Context error handling with proper fallback patterns
- **Dynamic UI states**: UI adapts to current AppState and GameSubState
- **Thread-safe integration**: UI responds to background image loading via channels

**Interactive Features:**
- **File dialog integration**: Runtime image selection with `rfd` crate
- **Real-time configuration**: Live puzzle parameter adjustment
- **Progress visualization**: Loading progress, generation progress, game completion
- **Multi-language support**: Japanese font support for Windows environments

### Development Context

**Current Architecture Status:**
- **Bevy 0.16.1**: Latest stable version with full feature support
- **Modular systems**: Split into logical modules for maintainability
- **Performance-first**: Event-driven, cached, and optimized for large puzzles
- **Thread-based I/O**: Non-blocking image loading with crossbeam channels

**Recent Major Features:**
- **Multi-selection system**: Box selection, multi-drag, smart deselection
- **Performance monitoring**: Comprehensive profiling and debug tools
- **Progressive generation**: Background puzzle piece creation
- **Advanced caching**: Multiple cache layers for optimal performance
- **Thread-based loading**: Responsive UI during image processing

**Dependencies and Features:**
- **Core**: `bevy` 0.16.1 with picking, state, and UI features
- **UI**: `bevy_egui` 0.35 for immediate mode GUI
- **Graphics**: `lyon` 1.0 for tessellation, `puzzle-paths` for jigsaw generation
- **Concurrency**: `crossbeam` 0.8 for thread-safe communication
- **I/O**: `rfd` 0.11 for file dialogs, `image` 0.25 for format support
- **Profiling**: Optional Tracy and Chrome tracing support

**Known Implementation Patterns:**
- **Event-driven optimization**: `PieceMoveCompleted`, `PiecePlacedEvent` for selective updates
- **Cache invalidation**: Smart refresh triggers for performance caches
- **State management**: Dual AppState/GameSubState for complex game flow
- **Component lifecycle**: Proper component addition/removal for placed pieces
- **Thread safety**: Crossbeam channels for main-thread communication

**Platform Considerations:**
- **Windows-first development**: Primary target platform with full testing
- **Cross-platform compatibility**: Linux and macOS support with known limitations
- **File dialog behavior**: Platform-specific file selection patterns
- **Asset path handling**: Absolute vs relative path conversion for Bevy compatibility

**Performance Characteristics:**
- **Target**: 60fps with 1000+ piece puzzles
- **Memory**: Efficient mesh caching and progressive loading
- **Responsiveness**: Non-blocking I/O and background processing
- **Scalability**: Event-driven systems scale with puzzle complexity

## Recent Implementation Notes

**Multi-Selection System:**
- Box selection with visual feedback and real-time preview
- Multi-piece dragging maintains relative positions
- Smart deselection when pieces are placed correctly
- Performance-optimized with caching and event-driven updates

**Thread-Based Image Loading:**
- Replaced async systems with `std::thread::spawn` for better compatibility
- Crossbeam channels for thread-safe communication
- Non-blocking UI during image processing and decoding
- External file registry for absolute path management

**Performance Optimization:**
- Event-driven piece placement checking (`PieceMoveCompleted` → `PiecePlacedEvent`)
- Multiple cache layers: piece selection, stroke meshes, position bounds
- Progressive puzzle generation with configurable spawn rates
- F12 runtime performance monitoring with multiple debug levels