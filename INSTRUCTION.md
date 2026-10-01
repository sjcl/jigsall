# Task: Implement GPU-based puzzle piece picking in Bevy

Puzzella のパズルピース選択処理を、CPU の AABB / polygon intersection ベースではなく、**GPU rasterization を利用した picking** として実装してください。

目的は、画面上に多数配置される任意形状のパズルピースについて、

- クリックした位置に存在するピースを取得する
- ドラッグした矩形領域に **1 pixel でも描画領域が入っているピースをすべて取得する**
- 複雑なピース形状について CPU 側に別の collision geometry を持たない
- 実際の描画形状と picking の結果を可能な限り一致させる

ことです。

## 基本方針

Bevy の ECS / renderer を使用しつつ、Render World 側に専用の GPU picking pass を実装してください。

矩形選択については、ID texture 全体を CPU に readback する方式ではなく、

**scissor rectangle + fragment shader + atomic bitset**

方式を第一候補としてください。

概念的には以下です。

```text
Main World
  PuzzlePiece(Entity)
    Mesh
    Transform
    PieceId

        ↓ extract

Render World

Selection Render Pass
  ↓
selection rectangle を scissor rect に設定
  ↓
対象となる puzzle mesh を描画
  ↓
fragment が生成された PieceId について
atomicOr(selection_bitset[word], bit)
  ↓
GPU storage buffer
  ↓
小さい bitset だけ CPU に async readback
  ↓
PieceId → Entity
```

## Piece ID

各選択可能なピースには、GPU picking 用の安定した整数 ID を割り当ててください。

例:

```rust
#[derive(Component)]
struct PuzzlePieceId(u32);
```

`Entity` の内部表現をそのまま shader に渡すことには依存せず、

```text
PieceId <-> Entity
```

を Main World 側で対応付ける設計にしてください。

ID 0 を「background / none」として予約する必要は、bitset方式ではありません。

ただし将来的に ID buffer 方式にも流用しやすい設計にしてください。

## GPU bitset

選択結果は GPU storage buffer 上の bitset に記録してください。

PieceId が `id` の場合、

```text
word = id / 32
bit  = id % 32
```

として、

```wgsl
atomicOr(&selection[word], 1u << bit);
```

相当の処理を fragment shader で行います。

実際の WGSL syntax / buffer declaration は、使用中の Bevy / wgpu バージョンに合う形で実装してください。

例えば10,000ピースでも必要な readback は約1.25KBなので、pixel buffer 全体を readback しないでください。

## Rectangle selection

ドラッグ選択時には、選択矩形を screen-space の scissor rectangle に変換し、

```rust
render_pass.set_scissor_rect(...)
```

相当を利用してください。

選択矩形外の fragment shader invocation を可能な限り発生させないことが重要です。

矩形の向きは、

- 左上 → 右下
- 右下 → 左上
- 左下 → 右上
- 右上 → 左下

のどのドラッグ方向でも正しく正規化してください。

viewport / window scaling / DPI / render target resolution が異なる可能性も考慮してください。

## Hit の定義

矩形との intersection は geometry の bounding box ではなく、

> picking pass で実際に fragment が1つ以上生成された

ことを hit と定義してください。

したがって、パズルピースの concave な形状や複雑な輪郭について、CPU 側で polygon intersection を実装しないでください。

通常描画で alpha discard / alpha mask を使用して形状を作っている場合は、picking shader 側でも同じ基準を使用してください。

例えば概念的には、

```wgsl
let color = textureSample(...);

if color.a < ALPHA_THRESHOLD {
    discard;
}

atomicOr(...);
```

のようにします。

可能な限り、

```text
visible geometry
=
pickable geometry
```

となるようにしてください。

## Overlapping pieces

矩形選択では、他のピースに隠れているピースも含め、

**選択矩形に geometry が入っているすべてのピース**

を取得したいです。

そのため picking pass では通常描画の depth / draw order によって fragment が消えないようにしてください。

たとえば、

```text
Piece A
Piece B が A の上に重なっている
```

場合でも、矩形が両方に重なっていれば A / B の両方を返してください。

必要なら depth test を無効化してください。

2D の描画順序のために通常描画側で depth / z ordering を使用していても、rectangle picking の結果には影響させないでください。

## Point / click picking

クリックによる puzzle piece selection についても、既存の CPU picking から **GPU picking へ移行してください**。

Rectangle selection と click selection は、可能な限り同じ picking infrastructure、geometry、transform、camera、alpha mask 判定を共有してください。

ただし両者では selection semantics が異なります。

### Rectangle selection

選択矩形と実際に rasterized geometry が1 pixelでも重なる **すべての PieceId** を返してください。

この用途では、

```text
scissor rectangle
+
depth / occlusion 無効
+
atomic bitset
```

を使用します。

### Click selection

クリック位置でユーザーから実際に見えている **最前面の PieceId 1つ** を返してください。

概念的には、

```text
cursor position
    ↓
1x1 pixel の picking region
    ↓
puzzle geometry を rasterize
    ↓
通常描画と同等の alpha discard
    ↓
通常描画と同等の depth / draw order
    ↓
front-most PieceId
    ↓
GPU → CPU readback
```

としてください。

Click picking では rectangle selection の atomic bitset をそのまま使用して、CPU側で候補から最前面を推測する方式にはしないでください。

最前面判定自体を GPU 上で完結させてください。

### Rendering consistency

Click picking の hit 判定は通常描画と可能な限り一致させてください。

特に以下を共有してください。

- Mesh geometry
- Transform
- Camera / projection
- visibility
- alpha mask
- alpha discard threshold
- 必要な clipping
- front/back ordering

したがって、

```text
transparent pixel
```

をクリックした場合、そのピースは hit として扱わないでください。

別のピースがその背後に存在する場合は、その背後のピースが選択されることが期待されます。

### Overlapping pieces

例えば、

```text
Piece A
   ↑ partially covered by
Piece B
```

という状態で B がクリック位置を覆っている場合、

```text
Click:
    B only
```

としてください。

一方で rectangle selection では、

```text
Rectangle:
    A + B
```

となります。

この違いを明示的に設計してください。

### Readback

クリック結果についても CPU/GPU synchronization で render thread を block しないでください。

GPU 側で最終 PieceId を絞り込み、

```text
PieceId 1個
```

程度の非常に小さい結果だけを async readback してください。

画面全体の ID texture をCPUに転送してはいけません。

### Shared request API

可能であれば、

```rust
SelectionRequest {
    request_id,
    region,
    mode,
}
```

のように click / rectangle を共通化してください。

概念例:

```rust
enum SelectionMode {
    Point,
    Rectangle,
}
```

Point の場合:

```text
region = cursor position / 1x1
result = Option<Entity>
```

Rectangle の場合:

```text
region = rectangle
result = Vec<Entity>
```

ただし、型安全性や既存 architecture に適するのであれば PointSelectionRequest / RectangleSelectionRequest を分けても構いません。

### GPU implementation

Click picking の具体的な GPU 実装方法については、現在使用中の Bevy / wgpu version と既存 renderer を調査して最適な方法を選択してください。

候補には例えば、

- integer PieceId render target
- depth-tested ID rendering
- small GPU result buffer
- render order を考慮した GPU reduction

などがあります。

重要なのは、

1. 最前面判定を GPU 上で行う
2. full-screen pixel data を readback しない
3. 実際の通常描画と hit geometry を一致させる
4. clickごとの同期的 GPU wait を発生させない

ことです。

単純さと保守性を優先し、既存 Bevy renderer に最も自然に統合できる方式を選択してください。

## Final target architecture

最終的には以下を目標としてください。

```text
PuzzleSelectionPlugin
│
├── shared extraction
│     ├── Mesh
│     ├── Transform
│     ├── PieceId
│     ├── Camera
│     └── alpha / texture data
│
├── Point GPU Picking
│     ├── 1x1 selection region
│     ├── occlusion/order enabled
│     └── front-most PieceId
│
└── Rectangle GPU Picking
      ├── NxM scissor
      ├── occlusion/order ignored
      └── atomic PieceId bitset
```

CPU polygon intersection / AABB picking は、GPU picking が正常に動作した後は通常の puzzle selection path から削除してください。

Debug fallback として残す場合は明示的に feature flag または debug option として分離してください。

## Bevy integration

標準の `MeshPickingPlugin` に無理に組み込む必要はありません。

`PuzzleSelectionPlugin` のような独立した plugin としてまとめることを推奨します。

概念的には、

```rust
pub struct PuzzleSelectionPlugin;
```

内部に、

```text
Main World
├─ selection input/state
├─ PieceId allocation
├─ PieceId ↔ Entity mapping
└─ picking result handling

Render World
├─ extracted selection request
├─ extracted puzzle piece data
├─ GPU bitset buffer
├─ picking pipeline
├─ render graph node / render phase
└─ readback
```

を持たせてください。

現在利用している Bevy バージョンの API を確認した上で、

- RenderApp
- ExtractSchedule
- RenderGraph
- custom render phase
- render command
- GPU readback

などのうち、最も自然で保守しやすい API を選択してください。

古い Bevy バージョンの記事や example をコピーして、現行 API に存在しない型を使わないでください。

## Existing rendering data

可能な限り通常描画と以下を共有してください。

- vertex buffer
- index buffer
- mesh geometry
- transform
- texture / alpha mask
- per-piece instance data

Picking のためだけに CPU で同じ geometry を複製しないでください。

ただし、通常の Bevy material pipeline を無理に再利用することで実装が極端に複雑になる場合、

```text
normal rendering pipeline
picking pipeline
```

を分けても構いません。

重要なのは geometry / transform の source of truth を共通化することです。

## Instancing

現状または将来的に puzzle pieces を instancing できる構造なら、それを考慮してください。

例えば picking shader へ、

```rust
struct PuzzleInstance {
    transform: ...,
    piece_id: u32,
}
```

相当を渡せる構造が望ましいです。

ただし、このタスクのために既存 renderer 全体を大規模に instancing 化する必要はありません。

既存設計に自然に組み込める範囲にしてください。

## Selection request lifecycle

GPU readback は同期的に待たないでください。

Main Thread / render loop を、

```text
submit GPU work
↓
GPU finished?
↓
CPU block
```

のように止めないこと。

非同期 readback を使用してください。

選択要求には generation / request ID を付けてください。

例:

```rust
struct SelectionRequestId(u64);
```

これにより、

```text
request #10
request #11
request #12
```

と連続してドラッグされた場合、古い readback result が後から届いて現在の selection を上書きしないようにしてください。

少なくとも

```text
result.request_id == latest_relevant_request
```

を判断できる構造にしてください。

## Dragging behavior

ドラッグ中に毎フレームGPU readbackする必要があるかは検討してください。

第一案として、

- drag start
- drag update
- drag end

を区別し、

ドラッグ中は必要に応じて一定頻度で preview selection を更新し、

drag end では必ず最終 selection を要求する構造にしてください。

ただし premature optimization は不要です。

最初は毎 frame request でも構いませんが、

- 複数 request が in-flight になる
- readback latency が1フレーム以上ある

ことを前提に壊れない設計にしてください。

## GPU buffer management

毎回 GPU buffer を新規作成しないでください。

bitset buffer / readback に必要な resource は可能な範囲で再利用してください。

ピース数が増えて必要な bitset size を超えた場合のみ resize してください。

selection pass 前には bitset を必ず zero clear してください。

clear → render → copy/readback

の ordering が保証されるようにしてください。

## Empty / very small selection

以下を正しく処理してください。

- width == 0
- height == 0
- window 外に矩形がはみ出す
- 全体が viewport 外
- negative drag coordinates
- minimized / zero-sized window

invalid な scissor rectangle を wgpu に渡さないでください。

## Camera

Main puzzle camera の projection / viewport と picking pass の座標系を一致させてください。

特に、

```text
logical cursor coordinates
physical pixels
viewport offset
render target size
camera projection
```

を混同しないでください。

通常描画と picking で同じ transform / projection を使用し、カメラ移動・zoom 後も正しく選択できることを確認してください。

## Performance

重要なのは、以下のような実装を避けることです。

```text
矩形領域の全 pixel をCPUへ転送
```

または

```text
すべてのmeshについてCPUで triangle intersection
```

です。

期待する処理は、

```text
GPU:
    selection rect 部分だけ rasterize
    ↓
    atomic bitset

CPU transfer:
    O(number_of_pieces / 8 bytes)
```

です。

CPU readback量が selection rectangle の pixel 数に比例しないようにしてください。

## Correctness tests

最低限、以下を確認してください。

### 1. Basic

1ピースだけ矩形に重なる。

Expected:

```text
selected = [piece]
```

### 2. Partial overlap

ピースの1 pixel程度だけ矩形に入る。

Expected:

```text
selected = [piece]
```

### 3. AABB false positive

矩形が piece の bounding box には入っているが、実際の mesh geometry には触れていない。

Expected:

```text
selected = []
```

### 4. Multiple pieces

矩形内に複数 piece がある。

Expected:

すべて取得。

### 5. Occlusion

B が A を完全または部分的に覆っている。

Expected:

矩形が両方に重なっていれば A / B の両方を取得。

### 6. Camera zoom

camera zoom を変更。

Expected:

visual position と picking position が一致。

### 7. Camera translation

camera を移動。

Expected:

visual position と picking position が一致。

### 8. Drag direction

4方向すべてから矩形を作る。

Expected:

同じ領域なら同じ結果。

### 9. Transparent area

alpha mask された透明部分だけを矩形が通る。

Expected:

piece を選択しない。

### 10. Large piece count

少なくとも数千〜10,000程度の PieceId を想定した buffer sizing が正常に動くこと。

## Debugging

開発中のみ有効にできる debug visualization を用意すると望ましいです。

最低でも、

- normalized selection rectangle
- returned PieceId
- returned Entity

をログまたは debug overlay で確認できるようにしてください。

可能なら picking pass の結果を視覚的に確認する debug mode を追加して構いません。

ただし production path に unnecessary overhead を入れないでください。

## API

ゲームロジックから renderer の詳細を極力隠してください。

例えばゲーム側では概念的に、

```rust
SelectionRequest {
    rect: ScreenRect,
    mode: SelectionMode::Rectangle,
}
```

を送信し、

結果として、

```rust
SelectionResult {
    request_id,
    entities: Vec<Entity>,
}
```

を受け取る程度のAPIを目指してください。

実際の Bevy Event / Message / Resource / Observer の選択は、現在のプロジェクトで採用しているパターンに合わせてください。

## Implementation order

以下の順番で進めてください。

1. 現在の puzzle piece rendering architecture を調査
2. PieceId の割当方法を決定
3. selection request / result API を追加
4. Render World に必要なデータを extract
5. GPU atomic bitset buffer を作成
6. picking pipeline / WGSL shader を実装
7. rectangle scissor rendering を実装
8. async readback を接続
9. PieceId → Entity に戻す
10. selection UI と接続
11. camera / DPI / viewport edge cases を確認
12. overlapping / transparent geometry をテスト
13. resource reuse / unnecessary allocations を確認

各段階で compile / test してください。

一度に renderer 全体を書き換えないでください。

## Before modifying code

まず repository を調査して、以下を簡潔にまとめてください。

- 使用中の Bevy version
- puzzle pieces が現在どう描画されているか
- Mesh2d / Sprite / custom pipeline / instancing のどれを使っているか
- puzzle piece geometry の source of truth
- alpha / texture による discard の有無
- camera 構成
- 現在の selection / clicking implementation
- picking pass を追加するのに最適と思われる integration point

その後、その調査結果に基づいて実装してください。

設計がこの指示と既存コードで衝突する場合は、既存 architecture に自然に合わせることを優先してください。ただし、

**「矩形内に実際に rasterize される geometry が1 pixelでも存在するすべての PieceId を、GPUで集約して小さい結果だけCPUへ戻す」**

という基本方針は維持してください。

## Out of scope

今回のタスクでは以下は必須ではありません。

- physics engine 導入
- CPU polygon collision system
- generic picking framework の全面再設計
- renderer 全体の instancing 化
- multiplayer protocol の変更
- gameplay selection semantics の大幅変更

まず GPU rectangle picking を独立して正しく動かしてください。

## Deliverables

実装完了時には以下を報告してください。

1. 変更した architecture
2. 新規追加した主要 types / systems / render nodes
3. GPU picking の処理フロー
4. readback の仕組み
5. camera / coordinate conversion の扱い
6. overlap / transparency の扱い
7. performance 上の注意点
8. 残っている limitation
9. 実行した tests / checks
10. click picking を今後同じ仕組みに統合する場合の拡張ポイント

不要な大規模リファクタリングは避け、既存コードとの差分を小さく保ってください。