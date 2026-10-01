# Puzzella

任意の画像で遊ぶ、Rust + Bevy製のジグソーパズルゲームです。generator v3の解析形状をGPUで描画し、最大1000×1000ピースを扱います。ゲーム状態と命令検証はCPU側にあり、実ネットワーク通信は未実装です。

## 起動

Rust 1.95以上とOSに対応するC/C++リンカーが必要です。Windowsの詳細は[WINDOWS_BUILD.md](WINDOWS_BUILD.md)を参照してください。

```sh
cargo run --locked
cargo run --locked --release
```

1. メニューの「Game Setup」を選択します。
2. 「Select Image」でPNG / JPEG / WebP / BMPを読み込みます。
3. アスペクト比・目標ピース数・手動グリッドからサイズを設定し、seedとスナップ距離を調整します。
4. 「Start Game」で初期配置とdense stateを生成します。GPU bufferとpipelineの準備後、プレイに進みます。

同じ画像寸法・grid・seed・generator versionから、同じ整数形状パラメータ、安定PieceId、初期配置を再構成します。通常プレイはversion 3を要求します。v2は比較用featureとテストに残しています。異GPU間の浮動小数点・ラスタライズのbit一致は保証しません。

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

未選択のピースをクリックすると選択を置き換え、選択済みのピースをドラッグするとグループの相対位置を保って移動します。重なりでは手前の選択可能なピースを選びます。矩形は、範囲内にfragmentを持つ、隠れた選択可能ピースも含みます。alphaゼロの画像部分は選択しません。

UI上の押下では移動を開始しません。ポーズ・フォーカス喪失で保持を解放し、未確定の範囲選択を取り消します。正解位置の近くで離すとスナップし、配置済みピースはロックされます。全ピースの配置で完成画面へ進みます。

## 設計

Bevy 0.19.1 / bevy_egui 0.42を使用します。ピースごとのMesh、Handle、Entity、頂点・index bufferを作りません。16-byte dense stateからGPUで可視IDを生成し、4頂点のprocedural quadを1回のindirect drawで描きます。凸凹、画像UV、枠線、選択表示はshaderで計算します。

```text
Input → ClientCommand → CPU gameplay state → dirty ranges → GPU state
                                                          ↓
                                              visibility + indirect draw
                                                          ↓
                                                shared shape / picking
```

不透明画像はdepth test/write、半透明画像はGPU Z sortとalpha blendを使います。初期配置は中央の画像領域を避ける格子リングとseed付きshuffleでO(N)です。

| package | 責務 |
| --- | --- |
| `puzzella` / `src/` | 起動・プラグイン登録 |
| `puzzella-core` / `core/` | 安定ID、定義、命令検証、スナップ |
| `puzzella-game` / `game/` | 状態遷移、入力、dense state、GPU描画・選択、画像読み込み |
| `puzzella-puzzle` / `puzzle/` | v3形状のCPU参照、配置、grid、feature限定のv2生成 |
| `puzzella-ui` / `ui/` | egui画面 |

詳細は[ARCHITECTURE.md](ARCHITECTURE.md)、非同期選択は[GPU_PICKING.md](GPU_PICKING.md)、移行結果とメモリ内訳は[PROCEDURAL_RENDERER.md](PROCEDURAL_RENDERER.md)を参照してください。

## 検証・計測

```sh
cargo fmt --check
cargo check --locked
cargo clippy --workspace --locked --all-targets --all-features -- -D warnings
cargo test --locked
cargo test --locked --all-features
cargo build --locked
# 実GPU検証と1k〜1M計測
cargo test -p puzzella-game --release --locked gpu_ -- --ignored --nocapture --test-threads=1
# v2 / v3形状比較
cargo run --release --locked -p puzzella-puzzle --features cpu-geometry-reference --example shape_comparison -- target/shape-comparison.svg
```

2026-10-01、Windows / Rust 1.97 / RTX 5090（Vulkan）で通常37件と実GPU3件を確認しました。4096²画像・1024² offscreen・100万ピース全体表示は、不透明で平均2.17 ms、半透明で7.80 msです。同期GPU完了待ちを含むベンチマーク値で、通常ウィンドウのFPS保証ではありません。[報告書](PROCEDURAL_RENDERER.md)に計測条件と制限を記載しています。

## プロファイリング

```sh
cargo run --locked --release --features tracy
cargo run --locked --release --features chrome
```

Windowsではwgpu-halを29.0.3に固定しています。29.0.4とgpu-allocator 0.28のWindows COM型の不一致を回避するためです。`cpu-picking-debug`はv2のCPU triangle判定をビルドしますが、通常の選択はGPUです。
