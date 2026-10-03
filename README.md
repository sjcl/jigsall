# Puzzella

任意の画像で遊ぶ、Rust + Bevy製のジグソーパズルゲームです。generator v5の解析形状をGPUで描画し、最大1000×1000ピースを扱います。ゲーム状態と命令検証はCPU側にあり、opt-inのGNS Direct-IP transportとhost/peer routingを提供します。Host / Joinの画面接続は未実装です。

## 起動

Rust 1.95以上とOSに対応するC/C++リンカーが必要です。Windowsの詳細は[WINDOWS_BUILD.md](docs/WINDOWS_BUILD.md)を参照してください。

通信backendは `gns` featureで有効化します。native build依存、wire仕様、localhost Host + 2 clientsテスト、将来のSteamworks接続点は[NETWORK_TRANSPORT.md](docs/NETWORK_TRANSPORT.md)を参照してください。

```sh
cargo run --locked
cargo run --locked --release
```

1. タイトルの「New Game」を選択します。
2. 「Select Image」でPNG / JPEG / WebP / BMPを読み込みます。
3. アスペクト比・目標ピース数・手動グリッドからサイズを設定し、seedとスナップ距離を調整します。画像のプレビューと最終ピース数を確認できます。
4. 「Start Game」で初期配置とdense stateを生成します。GPU bufferとpipelineの準備後、プレイに進みます。

タイトルの「Settings」から解像度、Fullscreen / Borderless / Windowed、最大FPS（10〜1000、無制限あり）を変更できます。「Apply」で反映し、解像度・画面モードを変更した場合は15秒以内に「Keep Changes」で確定します。「Revert」または時間切れで元に戻ります。確定した設定は次回起動時も復元します。ボーダーレスはデスクトップの解像度を使用します。実装・保存先・検証方法は[DISPLAY_SETTINGS.md](docs/DISPLAY_SETTINGS.md)を参照してください。「Join Multiplayer」は未実装です。タイトル・新規ゲーム・ロード・セーブ・設定は共通の落ち着いた配色で表示し、小さいウィンドウではスクロールできます。ロゴは`assets/icon.svg`から変換したPNGを実行ファイルに同梱しています。

UI は English (`en-US`) と日本語 (`ja`) に対応しています。Settings の Language（設定 → 言語）で自動・English・日本語を選ぶと、再起動なしで表示が切り替わり、次回起動時も設定を復元します。自動では OS の言語を使用し、未対応の場合は英語に戻ります。翻訳カタログ、保存方式、日本語フォント、将来の Steam 接続については [LOCALIZATION.md](docs/LOCALIZATION.md) を参照してください。

同じ画像寸法・grid・seed・generator versionから、同じ整数形状パラメータ、安定PieceId、初期配置を再構成します。通常プレイはversion 5を要求します。v4の滑らかな付け根を保ち、辺の中心・幅・深さ・首と頭の比率・傾きに明確なクラスを持たせました。decodeと輪郭が変わるためv4を含む旧versionの定義は拒否します。v2は比較用featureとテストに残しています。異GPU間の浮動小数点・ラスタライズのbit一致は保証しません。

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
| 選択中、またはカーソル下の連結ピースを90°回転（反時計回り / 時計回り） | Q / E |
| pause / resume | Esc |
| プレイヤー表示 | Tab |
| パフォーマンス表示切替（FPSのみ → 詳細 → 非表示） | F3 |

キーボード操作は「設定 → キーコンフィグ」で変更できます。各操作にプライマリとセカンダリを設定でき、どちらでも操作できます。割り当て欄をクリックしてキーを押し、すべて離すと登録されます。Shift + RやA + Bなど、2キーの同時押しにも対応しています。左右のShift / Ctrl / Alt / Superは共通です。「適用」で保存し、次回起動時も復元します。「解除」で割り当てを空にし、「キー割り当てを初期値に戻す」で標準操作に戻せます。Escは固定で、入力待ちではキャンセル、それ以外では戻る・一時停止に使います。同じ割り当ての重複は保存できず、1キーとそのキーを含む2キーを押した場合は2キーの操作が優先されます。

未選択のピースをクリックすると選択を置き換え、選択済みのピースをドラッグするとグループの相対位置を保って移動します。重なりでは手前の選択可能なピースを選びます。矩形は、範囲内にfragmentを持つ、隠れた選択可能ピースも含みます。alphaゼロの画像部分は選択しません。

Q / E は選択中、またはドラッグ中の各componentを自身の表示位置の中心で回転します。選択がない場合はカーソル下のピース、またはその結合済みcomponent全体を回転します。ドラッグ中は表示中の移動を確定して基準位置を更新し、そのまま移動を続けられます。異なる回転角のピースは結合せず、盤面には0°のcomponentだけ配置できます。配置済みピースと他playerが保持中のcomponentは回転できません。実装と検証は[ROTATION.md](docs/ROTATION.md)を参照してください。

UI上の押下では移動を開始しません。ポーズ・フォーカス喪失で保持を解放し、未確定の範囲選択を取り消します。正解位置の近くで離すとスナップし、配置済みピースはロックされます。全ピースの配置で完成画面へ進みます。

完成画面の「View Completed Puzzle」で完成した盤面を閲覧できます。ホイールで拡大・縮小、右ドラッグで移動し、Escメニューの「Return to Title」でタイトルへ戻れます。完成画面から直接「Return to Title」を選ぶこともできます。

ポーズ・完成後のメニューの「Save Game」でタイトル（最大80文字）を入力してローカル保存できます。メインメニューの「Load Game」から保存を選ぶと、連結・配置・進捗を復元します。元画像は品質を変えずに保存されるため、元ファイルを移動・削除しても再開できます。保存済みゲームの通常 Save は同じ手動セーブを更新します。ホストでプレイ中は初期設定で5分ごとに別枠へオートセーブし、設定画面の一般タブで間隔（1–60分）の変更と無効化ができます。保存中はゲーム画面上部に表示し、ロード一覧では日時の横に「オートセーブ」と表示します。Steam Cloud は未実装です。保存先と形式は [PERSISTENCE.md](docs/PERSISTENCE.md) を参照してください。

元画像で正しく隣接し、同じ回転角のピースは、盤面外でも近い位置で離すと連結します。連結後はクリック・Ctrl・矩形選択・ドラッグで集合全体を扱い、正解位置へスナップすると全体が配置済みになります。複数の未連結集合も同時に選択・移動できます。

## 設計

Bevy 0.19.1 / bevy_egui 0.42を使用します。ピースごとのMesh、Handle、Entity、頂点・index bufferを作りません。16-byte dense stateからGPUで可視IDを生成し、4頂点のprocedural quadを1回のindirect drawで描きます。凸凹、画像UV、枠線、選択表示はshaderで計算します。

```text
Input → ClientCommand → CPU gameplay state → dirty ranges → GPU state
                                                          ↓
                                              visibility + indirect draw
                                                          ↓
                                                shared shape / picking
```

不透明画像はdepth test/write、半透明画像は可視IDだけを8bit × 3 passのGPU radix sortでZ順に並べてalpha blendします。初期配置は中央の画像領域を避ける格子リングとseed付きshuffleでO(N)です。

| package | 責務 |
| --- | --- |
| `puzzella` / `src/` | 起動・プラグイン登録 |
| `puzzella-core` / `core/` | 安定ID、定義、命令検証、スナップ |
| `puzzella-game` / `game/` | 状態遷移、入力、dense state、GPU描画・選択、画像読み込み |
| `puzzella-puzzle` / `puzzle/` | v5形状のCPU参照、配置、grid、feature限定のv2生成・fingerprint解析 |
| `puzzella-ui` / `ui/` | egui画面 |

詳細は[ARCHITECTURE.md](docs/ARCHITECTURE.md)、非同期選択は[GPU_PICKING.md](docs/GPU_PICKING.md)、透明sortの現行計測は[TRANSPARENT_RADIX_SORT.md](docs/TRANSPARENT_RADIX_SORT.md)、移行結果とメモリ内訳は[PROCEDURAL_RENDERER.md](docs/PROCEDURAL_RENDERER.md)、v4の付け根修正は[ROOT_TRANSITION.md](docs/ROOT_TRANSITION.md)、v5の識別性と単色プレビューは[EDGE_FINGERPRINT.md](docs/EDGE_FINGERPRINT.md)を参照してください。

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
# v2 / v5形状比較（6スタイルの凸・凹拡大も出力）
cargo run --release --locked -p puzzella-puzzle --features cpu-geometry-reference --example shape_comparison -- target/shape-comparison.svg
# 単色matching / 1000ピース / worst case / 輪郭識別性の測定
cargo run --release --locked -p puzzella-puzzle --features cpu-geometry-reference --example edge_fingerprint_preview -- target
# 無作為matching・5縦横比・各軸の実効寄与・人間向けHTML tool
cargo run --release --locked -p puzzella-puzzle --features cpu-geometry-reference --example edge_fingerprint_assessment -- target/edge-assessment
```

2026-10-01、Windows / Rust 1.97 / RTX 5090（Vulkan）で通常45件と実GPU3件を確認しました。v5の4096²画像・1024² offscreen・100万ピース全体表示のGPU drawは、3runの中央値で不透明0.5133 ms、半透明0.5189 msです。1024辺の最近傍輪郭距離はv4の約4.15倍になりました。[v5報告書](docs/EDGE_FINGERPRINT.md)に全クラス、preview、計測条件と制限を記載しています。

形状を変更せず識別性評価を強化した結果と、正誤・回答時間を記録するローカルHTML toolの使い方は[追加評価](docs/EDGE_FINGERPRINT_EVALUATION.md)に記載しています。4:1の長辺を64 px幅で表示すると隣接classの一部が同じmaskになり、高解像度の形状差と小さな表示での識別性を分けて扱う必要があります。評価追加後の通常49件とブラウザQAを確認しました。

## プロファイリング

```sh
cargo run --locked --release --features tracy
cargo run --locked --release --features chrome
```

Windowsではwgpu-halを29.0.3に固定しています。29.0.4とgpu-allocator 0.28のWindows COM型の不一致を回避するためです。`cpu-picking-debug`はv2のCPU triangle判定をビルドしますが、通常の選択はGPUです。
