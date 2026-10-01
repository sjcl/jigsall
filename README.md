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
| ボックス選択 | 空きスペースから左ドラッグ |
| 複数ピース移動 | 選択済みピースを左ドラッグ |
| カメラpan | 右ドラッグ |
| zoom | マウスホイール |
| edge scrolling | ピースをドラッグして画面端へ |
| pause / resume | Esc |
| プレイヤー表示 | Tab |
| 性能計測レベル切替 | F12 |
| バッチ再構築 / 統計 | F9 / F10 |

正解位置の近くでピースを離すとスナップし、全ピースを配置すると完成画面へ進みます。生成中・失敗時・ポーズ中・完成後はタイトルへ戻れます。

## 設計

Bevy 0.19.1 / bevy_egui 0.42へ更新しました。形状生成のpuzzle-pathsとlyon、同心円配置、カメラ、画像decode、egui設定UI、R-tree当たり判定、stroke cache、バッチ描画、性能計測を再利用しています。

```text
Input → ClientCommand → gameplay logic → PieceState → Transform / rendering
```

- `src/gameplay.rs`: 不変のPuzzleDefinition / PuzzlePiece、安定ID、可変PieceState、純粋な命令・スナップ判定
- `src/networking.rs`: backendに依存しないローカル命令の入口
- `src/game.rs`: Menu / GameSetup / InGame / GameCompleteと、InGame限定のInitializing / Playing / Paused
- `src/systems/`: 既存の生成・選択・カメラ・描画・性能システム
- `src/resources.rs`: 正本のピース記録とローカルの描画・当たり判定cache

元画像のtextureは1枚。ピースのMesh / UV / outlineと、共有する通常materialで描画します。スナップと進捗は一時描画Entityの有無に依存しません。

調査結果、変更・再利用・削除の理由、技術的負債、multiplayerの次の手順は[ARCHITECTURE.md](ARCHITECTURE.md)にまとめています。

## 検証

```sh
cargo fmt --check
cargo check --locked
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked
```

2026-10-01、Windows / Rust 1.97で上記チェックを通過し、8件のテストが成功しました。テスト対象は所有者検証、不正座標、スナップ閾値、seed付き形状・UV生成、1000ピース配置、ランダムfallback、Ctrl / box selectionとmulti-drag、Entityに依存しない完成判定、GamePluginの生成・ポーズ・完成・終了・再開始です。

`cargo build --locked`も成功しました。実行ファイルを8秒間起動し、初期化メッセージの出力とプロセスの継続、stderrにエラーがないことを確認して終了しました。画面の目視検証は行っていません。

実GPUでの1000+ピースのフレーム時間、ファイルdialogと全マウス操作、macOS / Linuxでの実行は追加確認が必要です。60fpsは測定済みの保証値ではありません。

## プロファイリング

```sh
cargo run --locked --release --features tracy
cargo run --locked --release --features chrome
```

Windowsではwgpu-halを29.0.3に固定しています。29.0.4とgpu-allocator 0.28のWindows COM型の不一致を回避するためです。
