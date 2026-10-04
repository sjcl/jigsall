# 開発ガイド

[Puzzella の紹介と起動方法](../README.md)に戻る。操作や保存・設定の使い方は [遊び方ガイド](PLAYING.md)、リポジトリの作業方針は [AGENTS.md](../AGENTS.md)を参照してください。

## ビルドと起動

最低対応 Rust はルートの [Cargo.toml](../Cargo.toml) の `workspace.package.rust-version`（現在 1.95）で管理します。OS に対応する C/C++ リンカーと、Bevy の描画 backend に対応する GPU・ドライバーが必要です。共通の依存定義と lockfile を維持し、リポジトリのルートで実行してください。

```sh
# 開発時の起動
cargo run --locked
# 最適化した構成で起動
cargo run --locked --release
# Direct-IP マルチプレイを有効にする
cargo run --locked --release --features gns
# rendezvous adapter と headless fixture（room-code UI は未統合）
cargo test --locked -p puzzella-game --features rendezvous rendezvous
```

Windows の MSVC toolchain・Visual Studio Build Tools・SDK と GNS のセットアップは [Windows ビルド手順](WINDOWS_BUILD.md)を参照してください。Linux CI で使うネイティブ依存の一覧は [CI の Install Linux native dependencies](../.github/workflows/ci.yml)にあります。GNS の依存と通信テストは [ネットワーク transport](NETWORK_TRANSPORT.md)にまとめています。

`gns` は CMake・Git・libclang などの追加のネイティブ依存を必要とし、`--all-features` でも有効になります。Windows では検証済みの LLVM/libclang 18.1.8 を使います。GNS の bindgen 0.70.1 と LLVM 23 は互換性がありません。ビルドキャッシュ・vcpkg 作業パスを共有する運用とロック待ちの扱いは [AGENTS.md](../AGENTS.md)に従ってください。

## リリース

master の履歴に含まれるコミットへタグを付けて push すると、[Draft release workflow](../.github/workflows/release.yml) が動きます。
Windows / Linux の x86_64 向けに `--locked --release --features gns` でビルドし、
両方が成功すると ZIP / tar.gz と `SHA256SUMS` を添付した GitHub Release の Draft を作成します。
アーカイブには実行ファイル、README、日本語フォントのライセンスを含めます。

```sh
git switch master
git pull --ff-only origin master
git tag v0.1.0
git push origin v0.1.0
```

master に含まれないコミットのタグとタグの削除はスキップします。過去の master のコミットも対象です。
失敗した場合は Actions から再実行できます。同じタグの Draft があれば添付ファイルを更新し、
公開済みの Release は変更せず失敗します。公開は GitHub の Releases 画面で Draft を確認してから行ってください。

## 構成と責務

Bevy 0.19.1 / bevy_egui 0.42 を使用します。依存バージョンはルートの `Cargo.toml` に集約しています。Windows 向けの wgpu-hal は 29.0.3 に固定しています。29.0.4 と gpu-allocator 0.28 の Windows COM 型の不一致を避けるためで、更新時には Windows での再ビルド確認が必要です。

| package / ディレクトリ | 責務 |
| --- | --- |
| `puzzella` / `src/` | 起動・プラグイン登録 |
| `puzzella-core` / `core/` | 安定 ID、定義、命令検証、スナップ |
| `puzzella-puzzle` / `puzzle/` | v1 形状（開発時 v5）の CPU 参照、配置、grid、形状評価用の fingerprint 解析 |
| `puzzella-game` / `game/` | 状態遷移、入力、dense state、GPU 描画・選択、画像読み込み・worker・通信 |
| `puzzella-ui` / `ui/` | egui の画面 |

ゲーム状態の正本は CPU の `PieceDataStore` です。入力からの命令を authority が検証し、確定した変更を GPU に渡します。

```text
Input → ClientCommand → CPU gameplay state → dirty ranges → GPU state
                                                          ↓
                                              visibility + indirect draw
                                                          ↓
                                                shared shape / picking
```

通常描画は procedural GPU renderer、選択は GPU picking です。ピースごとの Mesh・Handle・描画 Entity・頂点/index buffer を作らず、16-byte dense state から GPU で可視 ID を生成し、4 頂点の quad を 1 回の indirect draw で描きます。凸凹・画像 UV・枠線・選択表示は shader で計算します。不透明画像は depth test/write、半透明画像は可視 ID のみを 8 bit × 3 pass の GPU radix sort で Z 順に並べ、alpha blend します。

初期配置は中央の画像領域を避ける格子リングと seed 付き shuffle による O(N) の処理です。同じ画像寸法・grid・seed・generator version から整数形状パラメータ・安定 PieceId・初期配置を再構成します。回転設定もゲーム定義の一部です。

通常プレイは generator v1 を要求します。初回リリース向けに開発時 v5 の番号を 1 に整理し、形状・hash・seed・初期配置の計算は維持しています。開発時 v4 の滑らかな付け根を保ち、辺の中心・幅・深さ・首と頭の比率・傾きに明確なクラスを持たせています。対応する番号は 1 だけで、開発中の定義との互換性や移行は提供しません。旧 v2 の CPU メッシュ生成・CPU picking は削除しました。異 OS / GPU 間の浮動小数点・ラスタライズの bit 一致は保証しません。

`puzzella-puzzle` の `shape-analysis` feature は現行形状の fingerprint 解析と評価 example を有効にします。旧 CPU メッシュ生成・CPU picking の feature と専用 example はありません。通常描画・選択は GPU を使用します。現在の処理の詳細は [アーキテクチャ](ARCHITECTURE.md)を参照してください。

## 検証

変更した箇所に応じて、必要なローカル検証を実行してください。[GitHub Actions CI](../.github/workflows/ci.yml) は PR、`master` への push、手動実行に対応し、最低対応 Rust を `Cargo.toml` から読み取ります。

| CI の対象 | 確認内容 |
| --- | --- |
| 整形 | `cargo fmt --all --check` |
| Linux | default / all-features の Clippy（全 target）、テスト・doctest、ビルド |
| Windows | `gns` 有効の unit / integration test、アプリのビルド・リンク |
| GNS localhost | Linux / Windows で通信テストを直列実行 |

代表的なローカルコマンドです。Clippy には `cargo check` 相当の型検証も含まれます。

```sh
cargo fmt --all --check
cargo check --workspace --locked
cargo clippy --workspace --locked --all-targets -- -D warnings
cargo test --workspace --locked
cargo build --workspace --locked
```

全 feature の検証には、事前に GNS のビルド環境を用意してください。GNS の localhost テストはプロセス全体の singleton を使うため、ほかのテストと分けて直列実行します。

```sh
cargo clippy --workspace --locked --all-targets --all-features -- -D warnings
cargo test --workspace --locked --all-features -- --skip gns_localhost
cargo test --workspace --locked --all-features gns_localhost -- --nocapture --test-threads=1
cargo build --workspace --locked --all-features
```

実 GPU / ネイティブウィンドウを必要とする ignored テスト、実機 UI の確認、release での性能計測は、変更内容に応じてローカルで行います。通常の CI の結果だけで、実機の描画・入力・速度を検証したことにはなりません。

## GPU 検証・形状評価・計測

性能は release モードで測定します。F3 で FPS のみ・詳細・非表示を切り替えられます。

```sh
# 実 GPU 検証と 1k〜1M ピースの計測
cargo test -p puzzella-game --release --locked gpu_ -- --ignored --nocapture --test-threads=1
# 単色 matching / 1000 ピース / worst case / 輪郭識別性
cargo run --release --locked -p puzzella-puzzle --features shape-analysis --example edge_fingerprint_preview -- target
# 無作為 matching・5 縦横比・各軸の実効寄与・人間向け HTML tool
cargo run --release --locked -p puzzella-puzzle --features shape-analysis --example edge_fingerprint_assessment -- target/edge-assessment
```

プロファイリングは以下の feature で有効にします。

```sh
cargo run --locked --release --features tracy
cargo run --locked --release --features chrome
```

2026-10-01 の Windows / Rust 1.97 / RTX 5090（Vulkan）での検証・計測値は、[v5 報告書](EDGE_FINGERPRINT.md)に記録しています。4096² の画像、1024² の offscreen、100 万ピース全体表示の GPU draw は 3 run の中央値で、不透明 0.5133 ms、半透明 0.5189 ms でした。これは特定条件での描画時間であり、ゲーム全体のフレーム時間や別環境の性能を示す値ではありません。通常 45 件・実 GPU 3 件の検証と、1024 辺の最近傍輪郭距離が v4 比で約 4.15 倍になった評価も同報告書にあります。

形状を変更せず評価を強化した結果と、正誤・回答時間を記録する HTML tool は [識別性の追加評価](EDGE_FINGERPRINT_EVALUATION.md)を参照してください。通常 49 件とブラウザ QA の記録、および 4:1 の長辺を 64 px 幅で表示すると一部の隣接 class が同じ mask になる制限も残しています。これらの件数は当時の記録です。60 fps や異 OS の runtime 互換性は、個別の実測・実機検証なしに主張しないでください。

## 技術資料

| 分野 | 資料 |
| --- | --- |
| 現在の構成・責務 | [アーキテクチャ](ARCHITECTURE.md) |
| Remote cursor の描画・atlas・検証 | [GPU cursor presentation](REMOTE_CURSOR_GPU.md) |
| 描画移行の方針と結果 | [移行方針](INSTRUCTION.md)、[procedural renderer](PROCEDURAL_RENDERER.md) |
| GPU 選択・大量選択・透明描画 | [GPU picking](GPU_PICKING.md)、[100 万ピースの選択](MILLION_SELECTION.md)、[radix sort](TRANSPARENT_RADIX_SORT.md) |
| 形状と識別性 | [v4 の付け根修正](ROOT_TRANSITION.md)、[v5 fingerprint](EDGE_FINGERPRINT.md)、[追加評価](EDGE_FINGERPRINT_EVALUATION.md) |
| 連結・回転 | [connected snapping](CONNECTED_SNAPPING.md)、[回転](ROTATION.md) |
| 通信・同期 | [transport とビルド依存](NETWORK_TRANSPORT.md)、[Direct-IP runtime](DIRECT_IP_RUNTIME.md)、[rendezvous v1 adapter と実サーバー検証](RENDEZVOUS_V1.md)、[protocol](MULTIPLAYER_PROTOCOL.md) |
| 途中参加・ホスト移行 | [join in progress](JOIN_IN_PROGRESS.md)、[host migration](HOST_MIGRATION.md) |
| 保存・設定・表示・言語 | [保存](PERSISTENCE.md)、[設定](SETTINGS.md)、[表示](DISPLAY_SETTINGS.md)、[ローカライズ](LOCALIZATION.md) |

移行資料や計測報告は記録時点の内容を含みます。未実装の機能一覧として扱わず、変更前に現在のコードを確認してください。
