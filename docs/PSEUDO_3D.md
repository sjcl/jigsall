# 擬似3D描画

現在は hard shadow のみ実装しています。責務と描画順序は
[アーキテクチャ](ARCHITECTURE.md#pseudo-3d-presentation)を参照してください。

## 品質と screen-space LOD

`game/src/render/visuals.rs` の `PieceVisualQuality` resource を main world で変更できます。
初期値は High です。preset を `ResolvedPieceVisuals` へ変換し、extract で frame の LOD を判定します。
ユーザー向け UI と保存、Auto はまだありません。

| quality | projected 短辺の threshold | base offset | extra lift | opacity |
| --- | ---: | ---: | ---: | ---: |
| Low | 無効 | 0 px | 0 px | 0 |
| Medium | 14 px | 1 px | 3 px | 0.20 |
| High | 10 px | 1.5 px | 4.5 px | 0.25 |

offset は右下への正規化ベクトルに掛ける距離です。physical pixel 単位で、DPI による logical pixel とは
区別します。High の回転開始・中間・終了は 1.5 → 6 → 1.5 px です。
静止中も shadow を描きます。`elevation != thickness` であり、normalized elevation は追加 lift だけです。
side / thickness は将来、静止中にも存在する別パラメータとして追加します。
bevel、lighting、blur、実 Z elevation、drag / selection lift は今回の対象外です。

## 検証

```sh
cargo test --locked -p jigsall-game --lib
cargo test --release --locked -p jigsall-game gpu_shadow -- --ignored --nocapture --test-threads=1
cargo test --release --locked -p jigsall-game gpu_ -- --ignored --skip benchmark --nocapture --test-threads=1
```

`render/tests/shadow_tests.rs` は以下を実 GPU で確認します。通常 fixture と同じく storage buffer 上限を
stage あたり8に制限します。

- Low の矩形 flat output 全 pixel と、Medium / High の LOD 無効時の Low との pixel 一致
- animation が存在しない静止 piece の base shadow、shadow-only pixel の point / rectangle 選択除外
- 開始 / 中間 / 終了の separation、不透明な本体 pixel 不変、zoom / camera rotation 後の screen direction
- normal の透明画像領域と far splat の alpha 0 / 半透明 / 不透明
- 本体と通常 culling AABB が viewport 外でも shadow だけ見える境界
- 不透明 / 半透明の64 piece 重なりと同 rank 重複で alpha blend が一回に抑えられること
- 100万 piece 初期配置の Low / Medium / High で shadow draw 0、同一 pixel output、追加 upload 0

Q/Q・Q/E の既存 CPU retarget test は、境界の shadow offset も連続していることを確認します。
far shader の shadow 対応は将来 threshold を調整できるよう維持します。現行 preset の threshold は
far mode の1.5 pxより大きいため、far raster fixture だけ test 用の threshold 0.25 pxで実行します。
通常 production の far overview は shadow pass を完全 skip します。

lazy compilation の回帰2件は `synchronous_pipeline_compilation: false` で実行します。
queue 直後と準備中の frame を、次の frame に進めず texture から直接読み取ります。
Low → Medium / High、High の5 px → 11 px zoom、11 pxでの Medium → Highでは、
表示済み epoch の top が flat reference と全 pixel 一致し、準備完了後に shadow が追加されることを
確認します。初回表示と次の epoch のロードでは、要求した shadow を描くまで `RenderReady` を
進めないことも確認します。

`gpu_shadow_million_overview_low_high_skip_draw_benchmark` は release で12 frameの GPU visibility / draw
timestamp と `shadow_draws` を記録します。Low / Medium / High が同じ flat path を使うことの検証であり、
60 fps、別 OS / GPU の runtime 互換性、bit 一致の保証ではありません。

## 実機記録

2026-10-06、Windows / RTX 5090 / NVIDIA 610.88 / Vulkan、Rust 1.97。
workspace 通常テスト900件（doctest 1件を含む）、Clippy 全 target、整形検証が通過しました。
実 GPU の機能回帰36件は dev profile、新規 shadow 6件と100万 piece の計測 fixture は release で通過しています。
同日、lazy compilation の修正前 `1e674b0` で、queue 直後の frame が背景だけになることを
非同期 GPU fixture で再現しました。修正後は追加2件を含む実 GPU 回帰38件が dev profile で通過し、
通常テスト・Clippy 全 target・整形検証も通過しています。
1000² grid の実初期配置、512² offscreen、白の1×1不透明 texture、12 frame平均の短い1 runです。
導入前との速度比較や改善率ではなく、quality を変えても LOD 無効時の pass が増えないことを確認します。

| quality | visibility GPU (ms) | top draw GPU (ms) | shadow draws |
| --- | ---: | ---: | ---: |
| Low | 0.0256 | 0.4955 | 0 |
| Medium | 0.0254 | 0.4926 | 0 |
| High | 0.0264 | 0.4735 | 0 |

各 quality の全 pixel は Low と一致し、visible / indirect instance count は100万です。
state / root / rotation / remote mapping / remote delta / selection / drag upload はすべて0 bytes、
shadow pipeline cache も空です。計測は timestamp の短期変動を含み、quality 間の速度差を示しません。

既存 GPU 回帰の確認時、snapshot fixture の10,000-unit offset が現在の logical play area 外であること、
outline fixture の固定 AA margin が curved SDF の `fwidth` より狭いことを検出しました。
変更前 `6043474` の draw shader に一時的に差し替えて同じ4件の失敗を再現しています。
fixture を合法な400-unit offsetとCPU参照の2×2 pixel quadのAA boundへ更新し、runtime の
play area / selection outline / top shader の挙動は変更していません。
