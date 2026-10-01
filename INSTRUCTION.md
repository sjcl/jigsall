# Puzzella 開発再開指示

対象リポジトリ:
https://github.com/sjcl/puzzella

このリポジトリは、Rust + Bevy で作成途中だったジグソーパズルゲームです。既存コードを捨てて全面的に作り直すのではなく、現在実装済みの機能を最大限再利用しつつ、コードベースを整理し、将来的なマルチプレイ対応を前提とした構造へ段階的に移行してください。

## 最終的なゲームの方向性

任意の画像を読み込み、その画像をジグソーパズルのピースに分割し、各ピースを完成領域の周囲にばらばらに配置します。

プレイヤーはピースをドラッグしてパズルを完成させます。

最終的には以下をサポートしたいです。

- 任意画像からのパズル生成
- 数百〜1000ピース以上
- ジグソーパズル形状
- カメラのパン・ズーム
- 複数ピース選択・ドラッグ
- スナップ配置
- 途中参加可能なマルチプレイ
- クライアントの1人がホストになる host-authoritative multiplayer
- セッション作成・参加のための軽量な rendezvous server
- 可能ならクライアント同士の直接接続
- NAT越えできない場合のrelay fallback
- 将来的な保存・再開

ただし、今回の作業でマルチプレイ全体を完成させる必要はありません。

まず既存コードを健全な状態に戻し、マルチプレイを追加できる構造にしてください。

---

# 重要方針

## 1. 既存実装を最大限再利用する

以下は特に価値が高いため、明確な問題がない限り再実装しないでください。

- `src/jigsaw_shapes.rs`
- `puzzle-paths` を利用したジグソー形状生成
- lyon tessellation
- progressive puzzle generation
- `src/puzzle.rs` の初期配置ロジック
- カメラのパン・ズーム・edge scrolling
- 画像読み込み
- egui ベースのゲーム設定UI
- ピース選択・box selection・multi-drag
- スナップ判定
- performance/debug instrumentation

既存コードを理解せずに新しい実装へ置き換えないでください。

---

# 2. 最初にコードベースを調査する

変更を始める前に、リポジトリ全体を確認してください。

最低限確認するファイル:

- `Cargo.toml`
- `src/main.rs`
- `src/game.rs`
- `src/components.rs`
- `src/resources.rs`
- `src/puzzle.rs`
- `src/jigsaw_shapes.rs`
- `src/networking.rs`
- `src/systems/`
- `src/ui/`

特に以下を特定してください。

- 現在使われているシステム
- dead code
- legacy code
- 同じ役割を持つ新旧実装の重複
- state管理の重複
- `GameScreen`, `AppState`, `GameSubState` の関係
- `handle_piece_dragging_hybrid_legacy` が現在必要か
- networkingコードが現在ビルド対象になっているか
- キャッシュの責務
- ピース生成と描画の結合度
- ゲームロジックと入力処理の結合度

調査結果をもとに、安全に整理してください。

---

# 3. 最新Bevyへの更新

現在のコードは Bevy 0.16.1 を使用しています。

まず現在安定して利用可能なBevyバージョンを確認し、無理のない範囲で最新版へ更新してください。

関連crateも互換バージョンへ更新してください。

例:

- `bevy`
- `bevy_egui`
- `image`
- `rfd`
- `lyon`
- `lyon_tessellation`
- `serde`
- その他Bevy依存crate

ただし、依存更新そのものを目的に大規模な書き換えを行わないでください。

既存挙動を維持することを優先してください。

更新後は最低限、

```bash
cargo fmt --check
cargo check
cargo clippy
```

が通る状態を目指してください。

可能なら実行確認もしてください。

---

# 4. legacy / 重複構造を整理する

現在、旧実装と新実装が混在している可能性があります。

特に以下を整理してください。

- `GameScreen`
- `AppState`
- `GameSubState`
- legacy dragging system
- 古いselection処理
- 重複したinput state
- 未使用networkingコード
- 不要なcompatibility layer

state管理は可能な限り、

```text
AppState
  Menu
  GameSetup
  InGame
  GameComplete
```

と、

```text
GameSubState
  Initializing
  Playing
  Paused
  Complete
```

程度へ整理してください。

意味のない二重管理は削除してください。

---

# 5. PuzzlePiece の責務を整理する

現在の `PuzzlePiece` は、不変情報・描画情報・ネットワーク同期候補が混在しています。

将来のmultiplayerを考慮し、少なくとも概念的に以下を分離してください。

例:

```rust
#[derive(Component)]
pub struct PuzzlePiece {
    pub id: PieceId,
    pub grid_position: UVec2,
    pub correct_position: Vec2,
}
```

```rust
#[derive(Component)]
pub struct PieceState {
    pub position: Vec2,
    pub placed: bool,
    pub held_by: Option<PlayerId>,
}
```

描画専用情報:

```text
mesh
texture coordinates
bounds
shape
outline
```

などはネットワーク状態とは分けてください。

全てを厳密にこの型に合わせる必要はありませんが、

- immutable puzzle definition
- mutable gameplay state
- local rendering state

を分離してください。

---

# 6. 決定論的なパズル生成にする

マルチプレイでは全クライアントが同じパズルを再生成できるようにしたいです。

現在 `jigsaw_shapes.rs` では固定seed `42` が使われています。

これをゲーム設定から与えられるseedへ変更してください。

例えば:

```rust
pub struct PuzzleDefinition {
    pub seed: u64,
    pub grid_width: u16,
    pub grid_height: u16,
}
```

同じ `PuzzleDefinition` が与えられた場合、

- ジグソー形状
- ピースID
- 初期配置
- シャッフル順

が可能な限り同じ結果になるようにしてください。

`thread_rng()` に依存する箇所は、必要に応じてseed付きRNGへ変更してください。

例えば:

```rust
StdRng::seed_from_u64(seed)
```

を使用してください。

将来的にネットワークで全ピース形状や初期座標を送るのではなく、

```text
image
PuzzleDefinition
seed
```

だけで再構築できることを目標にします。

---

# 7. 入力処理とゲーム状態変更を分離する

現在、入力システムが直接 `Transform` や `PuzzlePiece` を更新している箇所がある場合、将来のhost-authoritative multiplayerに対応しやすい構造へ整理してください。

理想的には、

```text
Input
 ↓
Game Command / Intent
 ↓
Game Logic
 ↓
PieceState
 ↓
Rendering Transform
```

という流れにします。

例えば将来的に、

```rust
pub enum PieceCommand {
    Grab(PieceId),
    Move {
        id: PieceId,
        position: Vec2,
    },
    Release(PieceId),
}
```

を使える構造にします。

今回ネットワーク送信まで実装する必要はありません。

ただしゲームロジックがローカルマウス入力に直接依存しないようにしてください。

---

# 8. スナップ判定をauthoritative logicとして独立させる

現在存在する、

- `PieceMoveCompleted`
- `PiecePlacedEvent`
- placement / snap logic

は再利用してください。

ただし将来的にはホストが、

```text
ReleasePiece
 ↓
snap validation
 ↓
PiecePlaced
```

を決定できる構造にしたいです。

スナップ判定がUIやmouse inputに依存しないpure gameplay logicになるよう整理してください。

---

# 9. networking.rs は基本的に再設計対象

現在の `src/networking.rs` は、過去の試作として参考にはしますが、そのまま拡張しないでください。

特に以下の設計は引き継がないでください。

- 全ピース更新を `ReliableOrdered` で送る
- `Changed<PuzzlePiece>` をそのままネットワーク更新として扱う
- クライアントが送った `is_placed` をそのまま信用する
- serverがクライアントのPieceUpdateをそのままbroadcastする

将来的なネットワークモデルはhost-authoritativeとします。

概念的には:

```text
Client
  ↓
Command / Intent
  ↓
Host
  - ownership validation
  - movement validation
  - snap validation
  ↓
Authoritative State
  ↓
Clients
```

です。

通信種別は将来的に、

Reliable:

- Join
- Leave
- Grab
- Release
- Snap
- StartGame
- PuzzleDefinition
- Snapshot

Unreliable:

- Piece movement
- Cursor movement

などに分けたいです。

---

# 10. networking backendをゲームロジックから分離する

将来的には Lightyear または Quinn の利用を検討しています。

現時点ではどちらかへ強く依存させないでください。

可能ならprotocol概念を独立させてください。

例えば:

```rust
pub enum ClientCommand {
    GrabPiece(PieceId),
    MovePiece {
        id: PieceId,
        position: Vec2,
    },
    ReleasePiece(PieceId),
}
```

```rust
pub enum ServerEvent {
    PieceGrabbed { ... },
    PieceMoved { ... },
    PieceReleased { ... },
    PieceSnapped { ... },
}
```

これらはBevyのrendering componentとは分離してください。

---

# 11. 画像は1枚のtextureとして扱う設計を維持する

ピースごとに画像textureを生成しないでください。

可能な限り、

```text
original image texture × 1
+
piece mesh
+
UV coordinates
```

という構造を維持してください。

これはピース数が増えた場合のVRAM効率上重要です。

---

# 12. progressive generationは維持する

1000ピース以上でフレームを長時間ブロックしないよう、現在のprogressive generationの考え方は維持してください。

ただし実装が複雑化している場合は、

- CPU shape generation
- Bevy asset creation
- Entity spawning

の責務を明確に分離してください。

background threadからBevy WorldやGPU resourceを直接触らないようにしてください。

---

# 13. パフォーマンス最適化を壊さない

既存コードには、

- selection cache
- stroke mesh cache
- progressive generation
- event-driven placement
- performance monitor
- Tracy / Chrome tracing

があります。

不要と判断して削除する場合は、理由を確認してください。

単純化のために1000ピース以上で性能が大幅に悪化する変更は避けてください。

---

# 14. 過剰な再設計を避ける

今回は完成版アーキテクチャを一度に作ることが目的ではありません。

特に以下はまだ本格実装しなくて構いません。

- rendezvous server
- STUN
- TURN
- NAT traversal
- relay server
- reconnect
- account system
- matchmaking
- cloud persistence

まずローカルゲームを整理し、

```text
local input
↓
command
↓
authoritative-like game logic
↓
state
↓
rendering
```

の構造まで持っていってください。

---

# 推奨する作業順序

以下の順序で進めてください。

1. 現在のコードベースを調査
2. `cargo check` が通る状態を確認
3. Bevyおよび依存crateを更新
4. deprecated APIを修正
5. dead code / legacy codeを整理
6. state管理を整理
7. PuzzleDefinitionを導入
8. パズル生成をseed付きで決定論的にする
9. PuzzlePieceの状態・描画責務を分離
10. input → command → logic → state の構造に変更
11. snap logicを入力から独立
12. networking用protocol型の基礎だけ追加
13. 既存single playerが以前と同等に動作することを確認
14. README / architecture documentationを更新

---

# コード品質

以下を重視してください。

- Rustらしい型設計
- unnecessary cloneを避ける
- panic / unwrapの乱用を減らす
- 巨大なsystem functionを分割する
- Bevy ECSでResourceに何でも詰め込まない
- 描画状態とゲーム状態を混ぜない
- Entity IDをnetwork identityとして使用しない
- stableな `PieceId` / `PlayerId` を使用する
- public APIの責務を明確にする
- unnecessary per-frame iterationを避ける

ただし、性能上意味のないmicro optimizationは不要です。

---

# 既存機能の保持

リファクタ後も最低限以下が動作する状態を維持してください。

- アプリ起動
- メニュー表示
- 画像選択
- PNG/JPEG/WebP等の読み込み
- パズルサイズ設定
- ジグソー形状生成
- ピース生成
- 初期ランダム配置
- ピースドラッグ
- 複数選択
- box selection
- multi-drag
- 正解位置へのスナップ
- 進捗表示
- カメラpan
- zoom
- edge scrolling
- pause
- game completion

既存機能を削除して簡単にすることは避けてください。

---

# 作業時の判断

大きな変更を行う場合は、まず既存実装がなぜその構造になっているか確認してください。

特に `jigsaw_shapes.rs`、`piece_interaction.rs`、`puzzle_generation.rs` は比較的大きく、過去にパフォーマンス問題へ対応した結果である可能性があります。

単にコード量を減らす目的で書き直さないでください。

既存コードの意図を維持しながら整理してください。

---

# 最初の成果物

まず最初のフェーズでは以下まで進めてください。

- 現状アーキテクチャの確認
- 最新依存関係への更新
- コンパイルエラー解消
- legacy / dead codeの整理
- state構造の整理
- `PuzzleDefinition` とseedの導入
- 決定論的な形状・初期配置生成
- multiplayer追加に備えたデータモデル分離

この段階では実際のネットワーク通信はまだ実装しなくて構いません。

最後に、

- 何を変更したか
- 何を再利用したか
- 何を削除したか
- まだ残っている技術的負債
- multiplayer実装の次のステップ

をまとめてください。

既存コードが正常に動いている箇所は可能な限り維持し、段階的に変更してください。