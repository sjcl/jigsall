# Puzzella

任意の画像で遊ぶ、Rust + Bevy製のジグソーパズルゲームです。第1フェーズでは既存のローカルゲームを整理し、将来のhost-authoritative multiplayerへ接続できるデータと命令の境界を作りました。実ネットワーク通信は未実装です。

## 起動

Rust 1.95以上と、OSに対応するC/C++リンカーが必要です。Windowsの詳細は[WINDOWS_BUILD.md](WINDOWS_BUILD.md)を参照してください。

```sh
cargo run --locked
# 最適化版
cargo run --locked --release
```

1. メニューの「Game Setup」を選択します。
2. 「Select Image」でPNG / JPEG / WebP / BMPを読み込みます。
3. アスペクト比・目標ピース数・手動グリッドのいずれかでサイズを設定し、seedとスナップ距離を調整します。
4. 「Start Game」で生成します。CPU処理は背景スレッド、asset登録は10ピース/フレームで進みます。

同じ画像寸法・grid・seed・generator versionから、同じ形状・安定PieceId・初期配置を生成します。異機種や依存バージョン間の浮動小数点の完全一致は、今後の検証対象です。

## 操作

| 操作 | 入力 |
| --- | --- |
| 選択・ドラッグ | 左クリック / ドラッグ |
| 選択の追加・解除 | Ctrl + 左クリック |
| ボックス選択 | 空きスペースから左ドラッグ（Ctrl併用で追加） |
| 複数ピース移動 | 選択済みピースを左ドラッグ |
| カメラpan | 右ドラッグ |
| zoom | マウスホイール |
| edge scrolling | ピースをドラッグして画面端へ |
| pause / resume | Esc |
| プレイヤー表示 | Tab |
| 性能計測レベル切替 | F12 |
| バッチ再構築 / 統計 | F9 / F10 |

未選択のピースをクリックすると選択を置き換え、選択済みのピースをドラッグするとグループの相対位置を保って移動します。重なり部分では手前のピースを選択します。Ctrl + クリックは選択の追加・解除のみを行います。UI上の操作ではピース移動を開始しません。ポーズ・フォーカス喪失でドラッグを解放し、未確定の範囲選択は取り消します。

正解位置の近くでピースを離すとスナップし、全ピースを配置すると完成画面へ進みます。生成中・失敗時・ポーズ中・完成後はタイトルへ戻れます。

## 設計

Bevy 0.19.1 / bevy_egui 0.42へ更新しました。形状生成のpuzzle-pathsとlyon、同心円配置、カメラ、画像decode、egui設定UI、GPU picking、stroke cache、バッチ描画、性能計測を再利用しています。

```text
Input → ClientCommand → gameplay logic → PieceState → Transform / rendering
```

Cargo workspaceで次の責務に分けています。依存バージョンはルートの`Cargo.toml`、解決結果は共通の`Cargo.lock`で管理します。

| package | 責務 |
| --- | --- |
| `puzzella`（`src/main.rs`） | アプリの起動とプラグイン登録 |
| `puzzella-core` | 安定ID、パズル定義・状態、ClientCommand、純粋な命令検証・スナップ判定 |
| `puzzella-puzzle` | seed付き形状・配置・Mesh生成、グリッド計算。WorldやGPU rendererに依存しない |
| `puzzella-game` | Bevyの状態遷移、入力、非同期生成の制御、バッチ描画、GPU選択、画像読み込み |
| `puzzella-ui` | egui画面と`GameUiPlugin`によるUIシステム登録 |

`puzzella-game/src/resources/`は状態・設定・入力・生成・画像・ピース・描画・バッチ・性能・CPUデバッグ判定に分けています。`selection/`は要求API、座標変換、GPU描画・readbackを分離しています。詳細なファイル配置と依存方向は[ARCHITECTURE.md](ARCHITECTURE.md)を参照してください。

元画像のtextureは1枚。ピースのMesh / UV / outlineと、共有する通常materialで描画します。スナップと進捗は一時描画Entityの有無に依存しません。

調査結果、変更・再利用・削除の理由、技術的負債、multiplayerの次の手順は[ARCHITECTURE.md](ARCHITECTURE.md)にまとめています。

## 検証

```sh
cargo fmt --all --check
cargo check --workspace --locked
cargo clippy --workspace --locked --all-targets --all-features -- -D warnings
cargo test --workspace --locked
# ゲーム本体や描画をビルドせず、判定ロジックだけを検証
cargo test -p puzzella-core --locked
# 形状・UV・配置の再現性を検証
cargo test -p puzzella-puzzle --locked
```

2026-10-01、Windows / Rust 1.97で上記チェックを通過し、27件の通常テストが成功しました。GPU pickingのoffscreenテストもRTX 5090で別途成功しています。所有者検証、seed付き形状・UV生成、1000ピース配置、完成・セッション遷移に加え、GPUのbitset境界・座標正規化・遅延結果の競合、生成した三角形のクリック判定、手前側選択、矩形の辺交差、R-tree更新、Ctrl / 範囲選択 / 複数移動、解放位置でのスナップ、UI入力の抑制、フォーカス喪失・ポーズ時の解放、カメラ座標同期、バッチ抽出・返却時の描画順・UV・透明度を検証しています。

`cargo build --locked`も成功しました。実行ファイルを8秒間起動し、初期化メッセージの出力とプロセスの継続、stderrにエラーがないことを確認して終了しました。画面の目視検証は行っていません。

実GPUでの1000+ピースのフレーム時間、ファイルdialogと全マウス操作、macOS / Linuxでの実行は追加確認が必要です。60fpsは測定済みの保証値ではありません。

## プロファイリング

```sh
cargo run --locked --release --features tracy
cargo run --locked --release --features chrome
```

Windowsではwgpu-halを29.0.3に固定しています。29.0.4とgpu-allocator 0.28のWindows COM型の不一致を回避するためです。

## GPU selection

クリックはGPU上で最前面のIDを決定し、4 bytesを非同期readbackします。矩形はscissor内のfragmentをatomic bitsetへ集約し、隠れたピースも返します。10,000 IDのreadbackは1,252 bytesです。パズルカメラはMsaa::Offで通常描画とpickingの画素coverageを一致させます。

実装・座標変換・非同期入力・制限・検証手順は[GPU_PICKING.md](GPU_PICKING.md)を参照してください。実GPUの自動テストは次で実行できます。

```sh
cargo test -p puzzella-game --locked gpu_raster_selection -- --ignored --nocapture
```
