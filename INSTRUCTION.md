# タスク: 現在のnative lyon生成を基準に、100万ピース対応のProcedural GPU Rendererへ移行する

現在の最新実装を前提に作業してください。

基準コミット:

```text
22e0aa135c5bdc6a881a3fe2ab6d976087d728ba
perf: generate native puzzle geometry in parallel
```

このコミットでは既に以下が実装されています。

- `puzzle-paths` / SVG / `svgtypes` 廃止
- `EdgeId`
- stable hashによる`EdgeProfile`
- Round / Wide / Narrow / Deep / Shallow / Pear の6スタイル
- 共有辺の決定論的生成
- cubic Bezierによるnative contour
- lyon fill/stroke tessellation
- Rayon piece-level parallelism
- worker-local tessellator reuse
- 1段background worker
- generator version 2
- default buildでCPU triangle二重保持を削減
- `shape_hash` / stroke cache廃止
- U16 geometry
- GPU picking

これらを一度旧方式へ戻したり、再実装したりしないでください。

今回の目的は、現在の

```text
EdgeProfile
  ↓
Bezier
  ↓
lyon
  ↓
1 fill Mesh / piece
  ↓
1 stroke Mesh / piece
  ↓
Assets<Mesh>
  ↓
combined batch Mesh
```

を最終的に、

```text
PuzzleDefinition
        +
dense PieceState
        ↓
GPU storage buffer
        ↓
compute visibility
        ↓
visible PieceId list
        ↓
single procedural instanced draw
        ↓
shared quad
        ↓
fragment shader shape evaluation
```

へ置き換えることです。

目標規模は最大

```text
1000 x 1000
= 1,000,000 pieces
```

です。

---

# 1. 最重要方針

100万ピース時に以下を作ってはいけません。

```text
1,000,000 Mesh
1,000,000 stroke Mesh
1,000,000 render Entity
1,000,000 Handle<Mesh>
```

また、

```text
10 pieces / frame
```

でasset登録する現在の方式も廃止対象です。

最終状態ではパズルピース描画用の個別Meshはゼロにしてください。

理想的には頂点buffer自体も不要です。

```wgsl
@vertex
fn vertex(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
)
```

からquadを生成してください。

例えばtriangle stripなら、

```text
vertex count = 4
instance count = visible_piece_count
```

です。

---

# 2. 現行generator v2はすぐ削除しない

現在の

```text
puzzle/src/shapes.rs
puzzle/src/generation.rs
```

は、新方式を検証するための非常に有用なreference implementationです。

特に、

- EdgeId
- EdgeProfile
- 共有辺の向き
- style parameter ranges
- UV mapping
- shared edge tests
- seed variation tests

は再利用してください。

migration途中ではCPU lyon版をfeature/test限定で残してください。

例えば、

```text
legacy-mesh-debug
cpu-geometry-reference
```

等。

新GPU rendererの検証が完了する前に、現行generatorを一括削除しないでください。

最終的にruntimeからlyonを外せる状態になったら削除またはtest-only化してください。

---

# 3. generator version 3を導入する

今回shapeの数学的定義自体が変わるので、

```rust
GENERATOR_VERSION
```

を3へ更新してください。

generator v2:

```text
EdgeProfile
→ cubic Bezier
→ lyon polygon
```

generator v3:

```text
EdgeProfile
→ analytic/SDF procedural shape
```

と扱ってください。

同じseedでv2とv3の形状が異なることは問題ありません。

ネットワーク・保存データでは引き続き、

```text
generator_version
seed
grid_size
image_size
```

から形状を完全再構成できることを維持してください。

---

# 4. 現在のmix64はWGSL向けhashへ変更する

現在の`shapes.rs`には64bitの`mix64`があります。

WGSL/WebGPU側でu64整数演算に依存する設計にはしないでください。

generator v3では、RustとWGSL双方で簡単に同じ処理を書ける32bit integer hashを定義してください。

`seed: u64` は、

```text
seed_low:  u32
seed_high: u32
```

に分解して両方をhashへ参加させます。

例えば、

```rust
fn mix32(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}
```

のようなWGSLへそのまま移植可能なavalanche hashを使用できます。

具体的な式は調査して決定してください。

必須条件:

```text
seed low bits
seed high bits
orientation
edge x
edge y
parameter domain
```

すべてが結果に参加すること。

RustとWGSLで同一のraw integer profile parametersを生成するテストを作ってください。

浮動小数点そのもののbit一致ではなく、可能ならまずquantized integer parametersを一致させてください。

---

# 5. EdgeIdの概念はそのまま維持する

これは現在の設計をそのまま再利用します。

```text
Horizontal(x, y)
Vertical(x, y)
```

のcanonical shared edge IDを維持してください。

例えば、

```text
Piece(x,y).right
    =
Vertical(x+1,y)

Piece(x+1,y).left
    =
Vertical(x+1,y)
```

です。

GPUでも同じEdgeIdから同じEdgeProfileを生成してください。

4辺のshape paramsを100万ピース分保存する必要はありません。

---

# 6. EdgeProfileの意味も維持する

現在の

```rust
EdgeProfile {
    polarity,
    style,
    center,
    width,
    depth,
    neck_width,
    head_width,
    asymmetry,
}
```

という考え方はそのまま使用してください。

ただしv3ではBezier control pointsを生成するためではなく、

```text
analytic tab/blank shape
```

を生成するパラメータとして使用します。

現在の6スタイルも基本的には維持してください。

```text
Round
Wide
Narrow
Deep
Shallow
Pear
```

現行v2と大きく印象が変わらないよう、styleごとのwidth/depth/head/neck比率を初期値として再利用してください。

---

# 7. arbitrary cubicのpoint-in-pathをfragment shaderで行わない

fragment shader内で24〜32本のcubic Bezierに対してray crossing/root solveする方式にはしないでください。

100万ピース対応の目的と逆行します。

代わりに、tab/blankをGPU向けのanalytic shapeまたはSDFとして再定義してください。

例えば概念的に、

```text
piece
=
rectangle
union/subtract top tab
union/subtract right tab
union/subtract bottom tab
union/subtract left tab
```

です。

tab shapeは、

- rounded neck
- head ellipse/circle
- capsule
- rounded box
- smooth union

等の安価なprimitiveから構成してください。

重要なのは、

```text
width
depth
neck_width
head_width
center
asymmetry
```

が自然に反映されることです。

tabとblankは必ず同じanalytic shapeのunion/subtractionとして扱い、隣接ピース間で完全に補完してください。

---

# 8. 共通WGSL shape moduleを作る

main renderingとGPU pickingが別々のshape判定を持ってはいけません。

例えば、

```text
game/src/render/puzzle_shape.wgsl
```

などを作り、

```wgsl
edge_hash(...)
edge_profile(...)
sd_tab(...)
piece_signed_distance(...)
inside_piece(...)
piece_uv(...)
```

等を共通化してください。

通常描画shaderとselection shaderの両方から同じ実装をimportしてください。

目標:

```text
visible pixels
==
pickable pixels
```

です。

---

# 9. fragment shaderでshape clippingを行う

各ピースは最大tab depthまで含む共通quadとして描画します。

概念的には、

```text
        shared quad
┌─────────────────────┐
│                     │
│   ┌─────────────┐   │
│   │ nominal     │   │
│   │ piece rect  │   │
│   └─────────────┘   │
│                     │
└─────────────────────┘
```

です。

fragment shader:

```wgsl
let d = piece_signed_distance(local_position, ...);

if d > 0.0 {
    discard;
}
```

としてください。

AAが必要なら、

```wgsl
fwidth(d)
```

を用いたcoverageを検討してください。

---

# 10. outlineもprocedural化する

現在は1ピースにつきfill Mesh + stroke Meshを作っています。

stroke Meshを完全に削除してください。

signed distanceが得られるなら、

```wgsl
abs(d) < outline_width
```

からoutlineを描けます。

現在の

```rust
stroke_width(size)
= min(16px, short_side * 0.16)
```

の視覚的挙動を参考にしてください。

これにより最終的には、

```text
StrokeTessellator
PieceStroke
stroke Handle<Mesh>
stroke Mesh
```

を削除できます。

---

# 11. EdgeProfileをfragmentごとにhashし直さない

これは重要です。

近距離で1ピースが数万fragmentを占有する場合、

```text
fragmentごとに
4 EdgeId
→ hash
→ 4 EdgeProfile
```

を行うのは無駄です。

まずvertex shader側でPieceIdから4辺のEdgeProfileを生成し、flat varyingでfragmentへ渡す方式を検討してください。

例えば、

```wgsl
@location(...)
@interpolate(flat)
edge_top: ...
```

です。

parameter数が多い場合はquantize/packしてください。

例えば1 edgeを

```text
2 x u32
```

程度へpackできれば、

```text
4 edges = 8 u32
```

で済みます。

まず可読性の高い実装を作り、その後varying countとALUをprofileしてpackingしてください。

---

# 12. PieceごとのGPU stateは16 bytes前後を目標にする

例えば、

```rust
#[repr(C)]
struct GpuPieceState {
    position: [f32; 2],
    z_order: u32,
    flags: u32,
}
```

です。

16 bytesなら、

```text
1,000,000 pieces
= 16 MB
```

です。

flagsには必要に応じて、

```text
PLACED
SELECTED
PREVIEW
HELD
VISIBLE/ENABLED
```

等を格納してください。

PieceIdはstorage bufferのindexなので保存不要です。

以下も原則保存不要です。

```text
grid position
size
UV rect
edge parameters
bounds
```

grid position:

```text
x = piece_id % grid_width
y = piece_id / grid_width
```

size:

```text
PuzzleDefinitionから共通計算
```

UV:

```text
grid + local positionからshader計算
```

shape:

```text
seed + EdgeIdから生成
```

です。

---

# 13. CPU側PieceDataStoreをdense化する

現在、

```rust
HashMap<PieceId, StoredPieceData>
HashMap<PieceId, Transform>
```

を持っています。

100万ピースではこの構造を正本にしないでください。

PieceIdがrow-major dense integerなので、

```text
Vec / Box<[T]>
```

を利用してください。

少なくとも、

```text
PieceId -> HashMap lookup
```

を毎回行う構造は廃止してください。

また現在各pieceに保持している、

```text
PuzzlePiece {
    id,
    grid_position,
    correct_position,
    initial_position,
}
```

の多くは`PuzzleDefinition + PieceId`から計算できます。

100万個分保存する必要が本当にあるか調査してください。

可能ならCPU正本も、

```rust
struct RuntimePieceState {
    position: Vec2,
    z_order: u32,
    flags: u32,
}
```

に近づけてください。

`held_by`のように通常ほぼ全pieceでNoneの情報は、必要なら別のsparse structureへ分離してください。

ゲームロジックの正しさを優先しつつ、per-piece固定overheadを減らしてください。

---

# 14. 1,000,000個のBevy Entityを生成しない

現在、

```rust
commands.spawn(PuzzlePieceId(id))
```

などがあります。

procedural renderer移行後はper-piece render entityを作らないでください。

理想的にはパズル描画用Entityは、

```text
0〜数個
```

です。

temporary extraction Entityも最終的に不要です。

GPU shaderのflagsとz orderで、

- selected
- preview
- held
- normal

を描き分けてください。

---

# 15. BatchManagerを廃止する方向で移行する

現在の、

```text
per-piece Mesh
→ combined Mesh
→ extracted pieceだけtemporary Entity
→ selection変更でbatch rebuild
```

は100万ピース向けではありません。

procedural rendererが動作した段階で、

- `combine_meshes`
- `BatchManager`
- `BatchedMeshEntity`
- `BatchRebuildRequest`
- `temporary_entities`
- `create_temporary_entities`
- `cleanup_temporary_entities`
- `reconcile_piece_rendering`

を削除可能か調査してください。

通常描画は常にstorage bufferをsource of truthにしてください。

---

# 16. Z-orderをO(N log N)で再圧縮しない

現在、

```rust
next_z_order >= 50.0
```

になると全pieceをsortしてZを再割当します。

100万ピースでは避けてください。

GPU stateでは`u32 z_order`を使用してください。

初期値:

```text
z_order = PieceId
next_z = piece_count
```

grab時:

```text
z_order = next_z++
```

としてください。

shaderのdepthへ変換します。

f32 depthで整数順序が厳密に必要なので、24bit程度のexact integer rangeを考慮してください。

例えば約1600万回のfront operationまでは再圧縮不要です。

再圧縮が必要になっても、それは極めて稀なslow pathにしてください。

現在のように開始直後から100万pieceによってthresholdを超える設計にはしないでください。

---

# 17. GPU buffer更新はdirty pieceだけ行う

毎frame、

```text
1,000,000 PieceState
```

をmain worldからrender worldへclone/uploadしてはいけません。

initialization時のみfull uploadしてください。

通常frameでは、

```text
dirty PieceId
```

だけをrender worldへ渡してください。

連続PieceIdはrangeへcoalesceして、

```text
queue.write_buffer(...)
```

のcall数も抑えてください。

dirty数が一定割合を超えた場合だけfull uploadへ切り替える方式でも構いません。

BevyのExtractScheduleでも、100万要素のVecを毎framecloneしないこと。

---

# 18. GPU frustum/viewport cullingを導入する

100万instanceすべてを毎frameraster pipelineへ投入しないでください。

compute shaderで、

```text
PieceState[1,000,000]
       ↓
visibility test
       ↓
visible_piece_ids[]
```

を生成してください。

shapeそのものをcullingする必要はありません。

最大tab depthまで拡張したquad AABBでconservative cullingすれば十分です。

compute:

```wgsl
if piece_quad_intersects_view(...) {
    let dst = atomicAdd(&visible_count, 1u);
    visible_ids[dst] = piece_id;
}
```

のような構造で構いません。

---

# 19. indirect drawを利用する

visibility computeから、

```text
vertex_count   = 4
instance_count = visible_count
first_vertex   = 0
first_instance = 0
```

のindirect argsを作り、

```text
draw_indirect
```

してください。

目標として通常描画のdraw call数はpiece数ではなく、

```text
O(1)
```

にしてください。

---

# 20. vertex buffer / index buffer自体をなくす

共通quadの4頂点すらbufferに置かなくて構いません。

```wgsl
let corners = array<vec2<f32>, 4>(
    vec2(-1.0, -1.0),
    vec2( 1.0, -1.0),
    vec2(-1.0,  1.0),
    vec2( 1.0,  1.0),
);
```

等から`vertex_index`で生成してください。

最終的なpiece rendererには、

```text
Mesh
vertex buffer
index buffer
```

が不要な構成を目標とします。

---

# 21. UVもshaderで計算する

現在v2では、

```rust
(center + vec2(local.x, -local.y)) / image_size
```

を使用しています。

この意味論をそのままshaderへ移してください。

つまりtab部分もnominal cellを越えた元画像位置からsamplingします。

外周はstraightなのでimage外へ出ないことを確認してください。

UV rectを100万個保存しないでください。

---

# 22. GPU pickingを新rendererと統合する

現在のGPU pickingは既に良い基盤があります。

特に、

- asynchronous readback
- 1x1 point target
- rectangle bitset
- selectable bitset
- request ordering

は維持してください。

ただし現在は、

```text
RenderMesh
ATTRIBUTE_POSITION
ATTRIBUTE_UV_0
ATTRIBUTE_PIECE_ID
```

を描画しています。

これをprocedural drawへ変更してください。

selection passも、

```text
visible_piece_ids
PieceState buffer
PuzzleConfig
image texture
shared shape WGSL
```

を使います。

point picking:

```text
same inside_piece()
→ discard
→ PieceId output
```

rectangle selection:

```text
same inside_piece()
→ selectable check
→ atomicOr(bitset)
```

です。

現在実装済みの1x1 cropped click targetは維持してください。

---

# 23. selectable bitsetは維持してよい

100万ピースでも、

```text
1,000,000 bits
≈ 125 KiB
```

なので、現在のselectable bitset方式は十分軽量です。

これは無理に変更しないでください。

---

# 24. picking用visible listをmain rendererと共有する

可能ならmain rendering用compute culling結果、

```text
visible_piece_ids[]
```

をpickingでも利用してください。

clickごとに100万instanceをvertex shaderへ流すことは避けてください。

camera/viewが同じframeではvisible listを共有する設計を優先してください。

---

# 25. placementをO(N²)から変更する

現在の`puzzle/src/placement.rs`には、

```rust
is_overlapping_with_existing(...)
```

として既存position全件を線形走査する処理があります。

5000piece計測ですでにplacementが約36msを占めています。

100万pieceではこのアルゴリズムは使用不可です。

重複を「検査する」のではなく、最初から重複しないslotをconstructiveに生成してください。

例えば、

```text
central puzzle exclusion rectangle

その外側へ
rectangular rings / lattice
```

としてslotを順次作ります。

各slotはspacing上必ず非重複になるよう設計してください。

その後、

```text
seeded permutation / shuffle
```

でPieceIdへ割り当ててください。

最低条件:

```text
O(N)
```

または

```text
O(N log N)
```

です。

distance overlapの全件探索は禁止します。

1,000,000 piece placementのbenchmarkを追加してください。

---

# 26. `10 pieces / frame` asset登録を完全に削除する

現在、

```text
SpawningEntities
10 pieces / frame
```

なので100万pieceでは、

```text
100,000 frames
```

必要になってしまいます。

procedural rendererではasset生成自体がないため、このphaseを廃止してください。

最終的なgenerationは例えば、

```text
NotStarted
GeneratingState
UploadingGpu
Completed
Failed
```

程度にしてください。

CPU側のdense state完成後、一括GPU buffer作成を行い、描画可能になったらPlayingへ移行してください。

---

# 27. main threadに100万件のAssets操作をさせない

以下は全てpiece count非依存にしてください。

```text
Assets<Mesh>::add
Handle<Mesh>
Mesh2d
MeshMaterial2d
```

パズル画像textureは1つ。

render pipeline/bind groupも少数。

piece countに比例するのは、

```text
dense CPU state
GPU state buffer
visibility work
```

だけにしてください。

---

# 28. image alpha semanticsを確認する

現在のColorMaterialはBlendを使用しているため、任意の半透明PNGでは単純なdepth writeだけでは既存描画と完全一致しません。

ここを黙って壊さないでください。

まず入力画像がopaqueかをload時に判定できるか調査してください。

### Opaque image

こちらを100万piece向けfast pathにしてください。

```text
depth test/write
arbitrary instance order
```

で問題ありません。

### Arbitrary translucent image

必要なら当面、

- legacy renderer fallback
- visible instanceのGPU depth sort
- 別のtransparent path

のいずれかを採用してください。

今回の最適化のために「PNG alphaを無視する」という変更を暗黙に入れないこと。

---

# 29. dense CPU stateへの移行は段階的に行う

いきなりgameplay全部を書き換えず、以下の順番を推奨します。

## Phase A

procedural rendererを追加。

現行`PieceDataStore`からGPU stateを生成して見た目を一致させる。

legacy mesh rendererと切替可能にする。

## Phase B

GPU pickingをprocedural geometryへ切替。

見た目とpicking一致を確認。

## Phase C

Mesh/stroke/batching/temporary Entityを削除。

## Phase D

`PieceDataStore`をdense storageへ変更。

## Phase E

placementをO(N)化。

## Phase F

GPU culling + indirect drawを有効化し、100万piece benchmark。

各Phaseでテストを通してください。

---

# 30. legacy rendererとのA/B比較機能を一時的に用意する

開発中だけ、

```text
legacy lyon mesh
procedural GPU
```

を同じPuzzleDefinitionで切り替えられると検証しやすいです。

generator versionが異なる場合でも、styleと共有辺の性質を比較できるpreview/testを作ってください。

migration終了後は通常buildからlegacy pathを外してください。

---

# 31. tests

最低限以下を追加してください。

## Rust / WGSL hash parity

複数の、

```text
seed
EdgeId
orientation
```

に対してraw profile parametersが一致すること。

可能ならGPU testで確認してください。

## Shared edge complement

隣接pieceで同じEdgeIdを生成し、

```text
A tab
B blank
```

の境界がsample点で一致すること。

## Outer edge

外周が完全なstraight edgeになること。

## Style coverage

6 styleがseed range内ですべて出現すること。

## Shape safety

- finite
- no impossible parameter combinations
- neck < head
- tabがcornerへ侵入しない
- max depthを超えない

## UV parity

現行v2 UV式とv3 shader UV式がsample点で一致すること。

## Rendering/picking parity

描画されるfragmentだけがpoint/rectangle pickingされること。

tab先端、neck、blank内部を必ずtestしてください。

## Dense state indexing

```text
PieceId(n) == state[n]
```

を保証。

## Dirty upload

1piece移動時にfull million-entry buffer uploadが発生しないこと。

## Visibility

viewport外pieceがvisible listへ入らないこと。

tabだけviewportへ入るpieceはcullされないこと。

## Z ordering

重なったpieceで最大z_orderが描画・point pickingされること。

---

# 32. benchmark

最低限、

```text
1,000
10,000
100,000
1,000,000
```

piecesを測ってください。

CPU:

```text
initial placement
state initialization
GPU upload preparation
per-frame dirty sync
```

GPU:

```text
visibility compute
visible count
main draw
point picking
rectangle picking
```

メモリ:

```text
CPU state
GPU PieceState
visible ID buffer
selection bitsets
Mesh assets count
Entity count
```

を記録してください。

さらにcamera条件を分けてください。

### Near

```text
~1,000 visible
```

### Medium

```text
~10,000 visible
```

### Entire puzzle

```text
potentially 1,000,000 visible
```

特に「全体を引いて100万pieceが画面内」のケースを逃げずに測ってください。

---

# 33. 最終的な目標メモリモデル

概ね、

```text
CPU
Piece runtime state
    ~16-24 MB / 1M pieces

GPU
PieceState        ~16 MB
VisiblePieceIds   ~4 MB maximum
selection bitset  ~125 KB
selectable bitset ~125 KB
indirect/counters negligible
image texture     one
```

程度を目標にしてください。

100万個のMesh/Handle/Entityによる数百MB〜GB級のoverheadを発生させないでください。

---

# 34. 最終的な目標pipeline

```text
                 PuzzleDefinition
                        │
                        │
                O(N) placement
                        │
                        ▼
              dense PieceState[]
                        │
                 initial upload
                        │
                        ▼
             GPU PieceStateBuffer
                        │
          ┌─────────────┴─────────────┐
          │                           │
     dirty updates               compute culling
                                      │
                                      ▼
                              visible_piece_ids
                                      │
                              indirect args
                                      │
                    ┌─────────────────┴────────────────┐
                    │                                  │
              normal render                       GPU picking
                    │                                  │
            procedural quad                      same quad
                    │                                  │
        EdgeId → EdgeProfile                same EdgeProfile
                    │                                  │
               analytic SDF                     same SDF
                    │                                  │
           texture + outline                 ID / bitset
                    │
                    ▼
                  screen
```

---

# 35. 完了後に削除可能なもの

新rendererが完全に動作し、testsが通った段階で以下を整理してください。

runtime側:

```text
lyon
lyon_tessellation
MeshGeometry
PieceGeometry
TessellationWorker
FillTessellator
StrokeTessellator
per-piece Mesh
per-piece stroke Mesh
PieceStroke
BatchManager
combine_meshes
temporary piece entities
ATTRIBUTE_PIECE_ID
10-piece/frame asset registration
```

ただしreference/debug testsでlyonがまだ有用ならfeature限定で残して構いません。

不要なものを中途半端に両方式で永続的に維持しないでください。

---

# 36. 既存機能を維持する

以下を壊さないでください。

- click selection
- box selection
- Ctrl toggle selection
- multi drag
- delayed GPU selection response handling
- release position適用後のsnap
- focus loss release
- pause release
- placed piece lock
- camera pan/zoom
- selection preview
- same image UV behavior
- deterministic generation
- session cleanup
- future multiplayer向けPieceId安定性

GPU renderer導入を理由にgameplay logicをshaderへ移さないでください。

authoritative gameplay stateはCPU側です。

GPUはpresentation/picking acceleratorです。

---

# 37. 検証コマンド

既存の以下を維持してください。

```sh
cargo fmt --check
cargo check --locked
cargo clippy --workspace --locked --all-targets --all-features -- -D warnings
cargo test --locked
cargo test --locked --all-features
cargo build --locked
```

実GPU testも現在の環境で引き続き実行してください。

---

# 38. 作業結果で必ず報告するもの

最後に、

1. どの旧runtime構造を削除したか
2. 残したv2 reference code
3. v3 hash仕様
4. analytic shape/SDF仕様
5. `GpuPieceState` bytes/piece
6. CPU bytes/piece
7. 100万piece時の総CPU/GPU memory
8. Entity数
9. Mesh asset数
10. draw call数
11. visible culling方式
12. dirty upload方式
13. picking統合方式
14. placement計算量
15. 1k/10k/100k/1M benchmark
16. 全体表示時のframe time
17. 残った最大ボトルネック

を報告してください。

100万piece対応では「生成時間が速い」だけでは不十分です。

最優先KPIは、

```text
per-piece Meshなし
per-piece render Entityなし
O(N²)処理なし
通常frameでO(total pieces) CPU iterationなし
draw call O(1)
GPU upload O(dirty pieces)
```

です。