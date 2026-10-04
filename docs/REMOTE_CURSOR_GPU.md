# Remote cursor GPU presentation

Baseline: `de15c27808f78a60a5a534056f896e5aff33be83` (fetched master, 2026-10-04).
Branch: `codex/remote-cursor-gpu`.

## Data flow and camera ordering

```text
world-space CursorUpdate / CursorSnapshot (unchanged network presence)
  → RemoteCursorPresentation (Time<Real>, O(players) CPU smoothing)
  → displayed_world_position
  → ExtractSchedule: bounded GPU instances + coherent atlas reference
  → PrepareResources: upload changed instances and current view uniform
  → Core2d MainPass: puzzle → selection → cursor markers → cursor labels
  → egui HUD / menu / modal

PlayerRoster + localized fallback + window DPI + session identity
  → UI Last: resolve names only on relevant changes
  → ab_glyph rasterization + deterministic shelf packing
  → Image<R8Unorm> + PlayerId → UV / logical size (one atlas revision)
```

`extract_puzzle` computes the current main camera `clip_from_world`, physical
viewport size and origin after main-world camera updates and transform propagation.
Cursor preparation copies those exact fields from `ExtractedPuzzle`; it does not
project through a separate camera in the egui schedule. Both dedicated vertex
entry points multiply the smoothed **world** position by that matrix, then add
logical pixel offsets converted to physical pixels and NDC. Camera-only changes
therefore move pieces, markers and names in the same render frame, using only a
view-uniform update. Instance data contains no projected screen position.

The cursor node runs after `puzzle_node` in `Core2dSystems::MainPass`.
bevy_egui's 2D pass runs after that set, so HUD/menu/dialog drawing covers cursors.
Only Playing is extracted; paused/completion/menu states and pending transitions
away from Playing/InGame suppress the instances. Own and unknown PlayerIds and
nonfinite positions are filtered before upload. The GPU rejects every vertex of
an instance with an offscreen center, including labels that could reach into the
viewport, and both passes use the puzzle viewport and scissor. They have alpha
blending and no depth attachment or depth test/write.

## Buffers and draw calls

`GpuRemoteCursor` is a 48-byte storage-buffer entry:

| Offset | Value | Bytes |
| --- | --- | --- |
| 0 | smoothed world position, two f32 | 8 |
| 8 | label logical size, two f32 | 8 |
| 16 | deterministic PlayerId palette color, linear RGBA | 16 |
| 32 | atlas UV min/max, four f32 | 16 |

At most 64 remote instances occupy a fixed 3,072-byte buffer, allocated lazily.
Markers use one instanced triangle draw (3 vertices); labels use one instanced
quad draw (6 vertices). The dedicated `remote_cursor.wgsl` and pipelines have
their own bind layouts. No binding or branch was added to puzzle draw, visibility
or picking shaders. With no remote cursor, both draws and view upload are skipped.

The marker is 15 × 16 logical pixels. Label text uses a 12 logical pixel font,
with 4/2 logical pixel horizontal/vertical padding, a translucent black background
and white glyph coverage. Label origin is right/down from the pointer; size and
offset do not scale with camera zoom. The player palette retains the eight
existing colors; colors are derived locally from PlayerId, never sent on the wire.

## Label atlas and bounds

The UI uses `ab_glyph` (already in the dependency graph) with the existing
`ui/fonts/MPLUS1p-Regular.ttf` bytes, exposed by a single static fallback-font
catalog shared with egui. No second file or `include_bytes!` is introduced.
Latin and Japanese use that font, with pair kerning and rasterizer-provided bounds.
Missing glyphs resolve to U+FFFD, then `?`; an absent outline is harmless.
Names are restricted to 32 characters by both existing identity validation and
the presentation layout. Complex-script shaping is outside this presentation's
Latin/Japanese scope; this is not a general-purpose text engine.

Packing sorts by PlayerId and fills deterministic shelves, placing one transparent
texel around each rectangle to isolate linear sampling. Texture dimensions are
powers of two, with width at least 512 and just large enough for the widest name.
The atlas stores only `R8Unorm` glyph coverage, with no mipmaps. Background and
text colors are composed in the label fragment shader.

Explicit limits are 65 source labels, 32 glyphs per label, 4,096 pixels per axis
and **16 MiB per CPU bitmap / GPU atlas texture revision**. Layout holds at most
2,080 glyph records and bounded metadata; there is no per-player bitmap/texture.
Raster scale is clamped to 0.5–4× to bound extreme monitor scales; the shader still
uses the actual positive window scale to preserve logical size above that range,
with reduced sharpness. At 1×/2× the atlas uses 12/24 physical pixel font height.
Rounding can change logical text extents by up to one pixel. Overflow or unavailable
font parsing omits the atlas while retaining membership and cursor markers.

The cache key includes session identity, roster revision, resolved names, fallback
string, locale and window scale. Join/leave, rename, localization or DPI changes
rebuild; Menu teardown clears the cache even if a later session reuses names/IDs.
Ordinary frames do not layout/rasterize text. Cursor movement, smoothing and camera
pan/zoom are not atlas-cache inputs. Score-only roster changes can recheck names
without rebuilding the atlas if the key is identical.

Each rebuild creates a **fresh image handle** and publishes it together with UVs,
logical sizes and revision in one immutable Arc. Extraction retains that same Arc
while building its instances. A GPU image lookup always uses that revision's handle;
if upload is pending, only the label draw is skipped. It never uses new UVs with
an old texture. Marker/puzzle drawing and `RenderReady` do not wait for the atlas.
Old revisions follow Bevy asset/handle lifetimes and can briefly coexist in flight;
the 16 MiB bound is per revision, rather than an aggregate process-memory claim.

## Upload isolation and validation

Instance comparison covers the actual displayed world positions, color and label
metadata. Changed smoothing frames upload 48 × extracted remote count bytes, at
most 3,072; settled, idle and camera-only frames upload **0 instance bytes**.
Atlas images are immutable and are uploaded by Bevy once per rebuild; camera and
smoothing frames upload **0 atlas bytes**. The view uniform changes only for
camera/viewport/DPI changes. All cursor paths are independent of PieceDataStore,
piece selection/membership/connectivity and piece GPU dirty revisions.

Network implementation and CPU smoothing are unchanged: wire version 1,
Transient payload limit 1,280 bytes, 20 Hz heartbeat and full snapshot batching,
Ready/reorder gating, 400 ms expiry and PlayerRoster identity remain intact.
Cursor/atlas state does not enter authority cursors, GameSnapshot, checkpoints,
JoinBaseline or saves.

```powershell
cargo test --workspace --locked cursor_atlas -- --nocapture
cargo test --locked -p puzzella-game cursor -- --nocapture
cargo test --locked --release -p puzzella-game gpu_remote_cursor -- --ignored --nocapture --test-threads=1
```

CPU tests cover Latin/Japanese/fallback/unsupported and 32-character names, 65
labels, UV bounds, disjoint rectangles, deterministic ordering/bitmap, 1×/2×
raster resolution/logical extents, and presence/name/locale/DPI/reset cache changes.
The GPU camera test changes pan/zoom and reads the completed frame directly,
without advancing another app frame that could conceal a stale projection. It
checks piece/marker/name pixels, DPI sizing, offscreen culling and pause. The
million-piece GPU test drives 64 smoothed cursors under the canonical-state access
guard and checks unchanged PieceUpload revision and zero piece/root/selection/
drag/remote-mapping uploads, then zero cursor uploads when settled or camera-only.
An unavailable new atlas must leave markers and puzzle readiness intact.

2026-10-04, Windows / Rust 1.97: the three final atlas regressions passed, including
65 labels all containing 32 Japanese characters at 1×/2×/4× and a name-only change
with fixed roster revision. The workspace suite passed 839 unit tests and one
doctest (35 platform/GPU/benchmark cases remained ignored in that run).
The two release GPU regressions passed, including
1×/2× DPI, pan/zoom on the exact changed frame, an offset non-square viewport,
Japanese glyph coverage, own/offscreen filtering, pause and an unavailable atlas.
The million-piece fixture confirmed 3,072 instance bytes per moving 64-cursor
frame, zero instance bytes after settling and on camera-only frames, unchanged
atlas texture identity and zero piece/root/selection/drag/remote-mapping uploads.
`cargo build --workspace --locked`, `cargo fmt --all --check` and
`cargo clippy --workspace --locked --all-targets -- -D warnings` also passed.
These tests do not establish an FPS guarantee or runtime compatibility on other OS/GPU.
