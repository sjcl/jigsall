# Puzzella 第1フェーズのアーキテクチャ

## 調査結果と変更理由

更新前の作業ツリーは Bevy 0.16.1 / bevy_egui 0.35。`cargo check` は成功したが、未使用コードなどの警告が60件あった。作業開始時の未コミット変更（データストアからの当たり判定登録、頂点に基づくbounds修正、バッチ抽出・返却の修正）は調査対象に含め、目的を引き継いだ。

| 調査対象 | 更新前の構造・問題 | 第1フェーズの対応 |
| --- | --- | --- |
| `main.rs` / `game.rs` | 実際に登録される入力はUUID版。legacy版は登録されていなかった | 稼働経路を安定IDによる入力と命令処理へ接続。順序を明示 |
| state | `GameScreen` と `AppState` / `GameSubState` が同じ画面を二重管理 | `GameScreen`、起動直後にMenuへ遷移するだけのLoadingを削除 |
| ピース | 全体は `PieceDataStore`、操作時のみ一時Entity。しかしスナップ・進捗はEntityを参照 | 全ピースの正本をデータストアへ統一。Entityは描画表現 |
| 選択 | UUID版box selectionがEntityへの変換に依存し、未抽出のピースを選べなかった | R-treeの結果をPieceIdの選択集合へ直接反映 |
| バッチ描画 | 元のメッシュを捨て、boundsからUVを再計算する経路があった | 元のメッシュとUVを保持・共有。画像の凸凹部分の歪みを避ける |
| 生成 | shape worker → piece worker → 10ピース/フレームのasset登録 | 同じ段階構成を維持。キューはVecDeque、worker失敗を表示 |
| キャッシュ | Entityベースの旧選択キャッシュとID/R-tree方式が併存 | 未稼働のEntity境界キャッシュを削除。R-tree、stroke cache、ハイライトの変更検出を維持 |
| networking | `mod networking` とRenet依存が無効。クライアントの配置フラグを信用する試作 | 試作を削除し、稼働するローカル命令境界だけを追加 |

## 状態遷移

```text
AppState: Menu → GameSetup → InGame → GameComplete
              ↑                │          │
              └──── Menu ←──────┴──────────┘

InGame のみ存在する GameSubState:
Initializing → Playing ⇄ Paused
```

`GameSubState` はBevyの `SubStates`。InGameを離れると消える。完成は `AppState::GameComplete` だけで表す。完成時はパズルを残し、Menuへ戻ると全ピース・結合メッシュ・背景・選択枠・キャッシュ・worker受信器・命令・画像を清掃する。workerはWorldを触らず、受信器が破棄された場合は結果を捨てる。

## データの責務

| 型 | 責務 | ネットワーク共有の候補 |
| --- | --- | --- |
| `PuzzleDefinition` | generator version、seed、grid、画像寸法、snap距離。開始時に設定から固定 | ○ |
| `PuzzlePiece` | PieceId、grid位置、正解位置、初期位置。不変 | ○（定義から再生成可能） |
| `PieceState` | 現在位置、placed、held_by | ○ |
| `PieceRenderData` | bounds、形状、Mesh/Material handle | × |
| `PieceDataStore.transforms` | PieceStateから導くローカル描画座標と重なり順 | × |
| 選択・プレビュー集合 / `InputState` | ローカル選択、ドラッグoffset、マウス、カメラ | × |
| `BatchManager` / `PieceIdManager` | 結合メッシュと一時Entityのローカル管理 | × |

ピースIDはgridのrow-major `u32`。プレイヤーIDは `u64`。どちらもEntityと独立し、serdeに対応する。PieceIdのスコープは1パズルなので、将来の通信ではsession/puzzle identityを併せて検証する必要がある。

## 入力から描画まで

```text
mouse / box selection / multi-drag
  → ClientCommand { player, PieceCommand }
  → apply_piece_command（所有者・配置済み・有限座標の検証）
  → PieceState
  → Release後の PieceMoveCompleted
  → snap_piece（純粋なゲーム判定）
  → PiecePlacedEvent
  → 全ピースに対する進捗・完成判定
  → 変更IDのみ Transform / R-tree を更新
  → 選択・保持中ピースを既存バッチから抽出して描画
```

入力はゲーム座標のTransformを書き換えない。命令処理はマウスやUIを参照しない。受理されたReleaseだけがスナップを起動し、閾値は従来同様 `distance < snap_distance`。配置済みのピースは再Grabできない。ポーズ開始とボタンの押下終了でローカル保持を解放する。

スナップと完成判定に一時Entityは不要。配置済みピースも結合メッシュに残す。一時Entityから状態を逆同期する経路は削除した。

## 再現性と描画

`puzzle-paths`、SVG処理、lyon tessellation、初期の同心円・フォールバック配置を再利用。形状seedはライブラリの `usize` APIに合わせ、64-bit seedをfoldして24-bitへ制限する（32/64-bitで同じ値、異なるseed同士で形状が重複することはある）。初期配置のフォールバックとshuffleは同じ `ChaCha8Rng` のseed付きストリームを使用する。

生成順・PieceId・バッチ結合順を固定。形状、頂点、indices、UV、初期座標が同じ定義から再現されることをテストする。generator versionは1で、Cargo.lockも成果物の一部。`puzzle-paths` の浮動小数点sin、tessellation、乱数crateの更新による異機種・異バージョン間のbit一致までは保証しない。

元画像のImageは1枚、通常マテリアルは共有。各ピースの元メッシュはCPUからも参照可能なassetとして保持し、結合描画・抽出描画で同じUVを使う。stroke mesh cacheと共有outline materialを継続利用する。背景画像も同じImage handleを参照する。

## 更新した依存関係

2026-10-01時点の安定版 [Bevy 0.19.1](https://docs.rs/crate/bevy/0.19.1) と対応する [bevy_egui 0.42](https://docs.rs/crate/bevy_egui/0.42.0) に更新。Rustの最低版はBevyに合わせ1.95。buffered EventはMessageへ、rendererの移動した型とeguiのPanel APIを移行した。image、serde、lyonなどは最新の互換版へlockfileを更新。未使用のRenetコメント依存、uuid、bincodeを削除した。

Windowsではwgpu-hal 29.0.4とgpu-allocator 0.28のWindows COM型が一致しなかったため、互換性を確認したwgpu-hal 29.0.3へCargo.tomlで限定固定している。

## 削除したもの

- 未登録の `handle_piece_dragging_hybrid_legacy`、旧box selection、旧毎フレームplacement/progress、重複reset処理
- `GameScreen`、`needs_reset`、`use_target_mode`、未使用UIマーカー・バッチ状態型、空の `ui/common.rs`
- Entity依存の未稼働選択キャッシュ、二重のドラッグ状態、逆方向のTransform→gameplay同期
- 元UVを捨てるメッシュ再生成と、使用されない旧SVG parser / 重複hit test
- Renet試作通信、動作していないJoin/Port画面、不要なID登録の旧システム
- worker間で共有されていた `static mut` のログ用カウンター

稼働中の形状生成、カメラpan/zoom/edge scrolling、画像decode、egui設定、R-tree/triangle hit test、stroke cache、イベント駆動スナップ、性能計測、Tracy/Chrome tracingは再利用している。

## 残る負債と次のステップ

1. 構造体は役割を分けたが、PieceDataStore内で正本とローカルcacheをまとめている。snapshot APIと描画adapterを次の段階で分離し、古い命令のsession ID / sequence / ownershipを検証する。
2. ローカル操作では有限座標と所有者を検証する。ネットワーク向けの移動速度・座標範囲・レート制限、切断時の保持解放、拒否応答は未実装。
3. 大量選択のプレビューで抽出数が変わると結合メッシュを再構築する。1000+ピースの実GPU計測を行い、必要ならchunk単位の結合とoverlay専用描画を導入する。
4. CPUメッシュを保持するためメモリは増える。単一textureというVRAM方針は維持するが、形状cacheとmeshコピーは次の計測対象。
5. 画像dialogは同期、decodeはworker。cross-platformのdialog経路と画像エラー表示、極端なアスペクト比・巨大grid・極小画像は追加検証が必要。
6. Lightyear/Quinnを比較する前に、定義＋画像hash＋snapshot＋host確定イベントのprotocolを設計する。Join/Grab/Release/Snap/SnapshotはReliable、Move/CursorはUnreliable候補。途中参加ではホストsnapshotを正本にする。
7. rendezvous、NAT越え、relay、保存・再開は次フェーズ以降。現在は実ネットワーク通信を実装していない。

## 検証結果

2026-10-01、Windows / Rust 1.97で `cargo fmt --check`、`cargo check --locked`、`cargo clippy --locked --all-targets --all-features -- -D warnings`、`cargo test --locked`、`cargo build --locked`が成功。8件のテストで以下を確認した。

- 所有者の異なるGrab / Move / Release、NaN座標を拒否し、スナップの従来閾値と配置済みロックを保持
- 同じ定義からPieceId・形状頂点・indices・UV・初期位置が再現され、別seedで配置と形状が変わる
- 1000ピースの配置数・有限座標・除外領域・再現性、ランダムfallbackの再現性と非重複
- 一時EntityがなくてもCtrl / box selectionとmulti-dragが相対位置を保ち、Releaseで保持・R-tree除外を解消
- 全ピースのスナップ・進捗・完成と結合メッシュ内の配置済みピース保持
- 実GamePluginで生成worker → Playing → Paused → Playing → GameComplete → Menu → 再生成し、定義・画像・バッチ・SubStateを清掃

実行ファイルは8秒間の起動確認で初期化メッセージを出力し、継続動作した。stderrにエラー出力はなく、その後終了した。画面の目視確認、実ファイルdialogでの画像選択、全マウス操作、1000+ピースのGPU性能、macOS / Linuxは未検証。操作・再現手順はREADMEに記載する。
