# 擬似3D描画

現在は hard shadow、厚紙の side / thickness、top surface の bevel / fake lighting を実装しています。責務と描画順序は
[アーキテクチャ](ARCHITECTURE.md#pseudo-3d-presentation)を参照してください。

## 品質と screen-space LOD

`game/src/render/visuals.rs` の `PieceVisualQuality` resource を main world で変更できます。
初期値は High です。preset を `ResolvedPieceVisuals` へ変換し、extract で frame の LOD を判定します。
ユーザー向け UI と保存、Auto はまだありません。

| shadow quality | projected 短辺の threshold | base offset | extra lift | opacity |
| --- | ---: | ---: | ---: | ---: |
| Low | 無効 | 0 px | 0 px | 0 |
| Medium | 14 px | 2.5 px | 3 px | 0.20 |
| High | 10 px | 3 px | 4.5 px | 0.25 |

| side quality | projected 短辺の threshold | static thickness | neutral linear RGB | opacity |
| --- | ---: | ---: | ---: | ---: |
| Low | 無効 | 0 px | 0 | 1 |
| Medium | 22 px | 1 px | (0.10, 0.10, 0.10) | 1 |
| High | 14 px | 1.5 px | (0.06, 0.06, 0.06) | 1 |

| bevel quality | projected 短辺の threshold | width | highlight strength | shadow strength |
| --- | ---: | ---: | ---: | ---: |
| Low | 無効 | 0 px | 0 | 0 |
| Medium | 28 px | 1 px | 0.06 | 0.09 |
| High | 18 px | 1.5 px | 0.10 | 0.14 |

offset は右下への正規化ベクトルに掛ける距離です。physical pixel 単位で、DPI による logical pixel とは
区別します。共通の `PSEUDO_3D_DIRECTION` は camera / piece の回転に影響されません。
High の shadow は回転開始・中間・終了で 3 → 7.5 → 3 px、side は常に1.5 pxです。
静止中も shadow を描きます。`elevation != thickness` であり、normalized elevation は追加 lift だけです。
side は静止中にも存在する別パラメータです。shadow と side の threshold は独立し、High の12 pxでは
shadowだけを描きます。Low / LOD off では optional pseudo-3D pass 自体を発行せず、pipeline の新規
queue / raster / fragment もありません。静止時の影が側面の先へ残るよう base shadow の距離を調整しました。
bevel の LOD も独立し、High の16 pxでは shadow / side が有効でも bevel は無効です。
far zoom では threshold を下げても bevel を有効にしません。blur、実 Z elevation、drag / selection lift は対象外です。

## Top surface の bevel

`elevation != thickness != bevel` です。shadow は base + elevation × extra lift、side は static thickness、
top bevel は static な表面の明暗で、回転中も幅・強度を変更しません。bevel は既存 top fragment 内だけで
処理し、draw / pipeline variant / buffer / binding / texture / sampler を追加しません。
`PuzzleUniform` の visual cull / side padding を4 scalarへ置き換え、336 bytesのサイズを維持しています。

coverage / source alpha は従来の全辺SDFを使います。bevel は selection / preview と共有する
`outer_boundary_distance` で connected internal edge を除外します。完全に囲まれたpieceは有限な定数を
derivativeへ渡し、lightingをskipします。selection側のsentinelと既存outline判定は維持しています。

normal zoomでは外周distanceの `dpdx` / `dpdy` をdiscardとper-piece分岐より前、frame uniformの
`bevel_enabled` 分岐内で計算します。gradientの長さでdistanceをphysical pixelへ換算し、1 − smoothstep
で内側だけを補正します。screen-spaceのouter normalと `-PSEUDO_3D_DIRECTION` の内積により、
左上向きは少し明るく、右下向きは少し暗くします。piece / camera回転でも光源はscreen-space固定です。
linear RGBを白 / 黒へ控えめに寄せてclampし、alphaはそのままです。その後にselection / preview outlineを
適用するため、outline色が優先されます。画像alpha 0の領域にbevelだけを残しません。

Low / LOD offではgradient・length・smoothstep・lighting dot・RGB補正をskipし、far fragmentは
bevel処理前にreturnします。既存selection AAの `fwidth(d)` は維持します。top vertexのquarter-turn
fast path、shadow / sideの色、pick silhouette、main / pick cullingは変更しません。

## 描画と準備待ち

描画順は `shadow depth → shadow color → side → top` です。side は1回の indirect drawで、既存の
visible IDs / args / image texture / continuous pose / quarter-turn fast path を再利用します。
画像のRGBを貼り付けず、同じ SDF / source alpha で neutral color を描きます。opaque side は blendなしの
depth test / write、translucent side は既存の sorted IDs と alpha blendです。重なった半透明 side は
top と同じ source alpha の重なりとして合成し、neutral color に近づきます。
side / top の開始時に depth を別々に clear するため、top の depth semantics は変わりません。
結合部は元位置の top union で覆い、side 専用の connectivity scan / cache は追加しません。

初回 epoch は要求された全 optional pipeline を待ってから `RenderReady` を進めます。表示済み epoch
では準備中の feature だけを skip し、shadow / top / picking を継続します。失敗は renderer error として
記録します。main culling の `visual_cull_extent` は有効な最大 visual offset を含み、pick は top のみです。

## 検証

```sh
cargo test --locked -p jigsall-game --lib
cargo test --release --locked -p jigsall-game gpu_shadow -- --ignored --nocapture --test-threads=1
cargo test --release --locked -p jigsall-game gpu_side -- --ignored --nocapture --test-threads=1
cargo test --release --locked -p jigsall-game gpu_bevel -- --ignored --nocapture --test-threads=1
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

`render/tests/side_tests.rs` は、静止 side、独立LOD、回転開始 / 中間 / 終了の一定幅、zoom / camera回転、
connected union の内部継ぎ目、normal / far の alpha、opaque depth / translucent sort、side-only picking、
viewport端を実 GPU で確認します。shiftした flat top の参照 silhouette と全 pixel を比較し、side が
elevation と無関係な同じ offset を使うことを確認します。side-only viewport fixture は cull 拡張を確実に
通る10 px厚、far fixture は0.25 pxの test-only thresholdを使います。production preset は変更しません。
非同期 fixture はLow → High、Highの12 px → 20 px、Medium → High、新 epoch初回Highを確認します。
shadow専用 fixture はsideを無効にして従来の個別保証を維持します。storage buffer 上限は同じ8です。

半透明 connected fixture は、alpha 128 の2×1 / 2×2 component を同じ外形・透明度・回転中心の
1枚の矩形pieceと比較します。High side の静止 / 回転開始 / 中間 / 終了、shadow無効 / 有効で、
全pixelの各channelが参照より余分に暗くならないことを確認します（8-bit出力の差1を許容）。
topの背後にsideが透ける全体的な暗さは参照にも含め、結合境界での追加の暗い帯を検出します。

このfixtureはcameraを(0.25, 0.125) world unitずらし、45°の直線境界がpixel centerへ完全に
重なる条件を避けます。同条件をずらさず試すと、side無効のtopにも半透明の重複がありました。
2×1、alpha 128、回転0.060秒、camera / component中心(100,100)ではpixel(50,50)のRGBが
結合pieceで(255,136,136)、1枚参照で(255,187,187)でした。side有効時にも境界の差が残ります。
サブピクセル移動後にも回転終了時の2×2に参照より明るい孤立pixelがあるため、追加fixtureは
「余分な暗い帯」の回帰検証であり、全配置での継ぎ目解消や参照との完全一致を保証しません。
硬いSDF境界の重複・隙間は今回変更していません。

目視用に native 128² PNG を保存する場合は、`JIGSALL_VISUAL_PREVIEW_DIR` に出力先を指定して
`gpu_side` を実行します。Medium / High の静止状態、High と結合componentの回転中、
alpha 128の結合componentの回転中（shadow無効 / 有効）を保存します。
未指定のテストはファイルを書きません。非同期 fixture の最初の flat reference は Bevy の最終出力
pipeline も準備されるまで待ち、feature queue 直後の frame の読み取りは待たずに直接行います。

`render/tests/bevel_tests.rs` はLowの矩形全pixel、独立LOD / far強制無効、同じtop pipelineの再利用、
zoom / camera回転 / quarter-turn / continuous rotationでの光源方向とpixel幅、曲線tab / blankのCPU形状参照、
2×1 / 2×2 / 3×3結合の内部辺（alpha 128と全4辺connected memberを含む）、linear RGB合成と
alpha 0 / 128 / 255、画像alpha穴、
selection / preview優先、同じposeでGPU rotation recordのstart elevationだけを変えたpixel一致、
bevel ON/OFFのpoint / rectangle一致と追加upload 0を実GPUで確認します。
shadow / side専用fixtureはbevelを無効にし、既存の個別保証を維持します。
`gpu_bevel` は同じ環境変数でMedium / High静止、High回転中、High結合回転中のPNGを保存できます。

lazy compilation の回帰2件は `synchronous_pipeline_compilation: false` で実行します。
queue 直後と準備中の frame を、次の frame に進めず texture から直接読み取ります。
Low → Medium / High、High の5 px → 11 px zoom、11 pxでの Medium → Highでは、
表示済み epoch の top が flat reference と全 pixel 一致し、準備完了後に shadow が追加されることを
確認します。初回表示と次の epoch のロードでは、要求した shadow を描くまで `RenderReady` を
進めないことも確認します。

`gpu_shadow_million_overview_low_high_skip_draw_benchmark` は release で12 frameの GPU visibility / draw
timestamp と `shadow_draws` / `side_draws` を記録します。全 quality で両draw 0、両pipeline cache未生成、
piece / mapping / animation / selection / dragの追加upload 0、同じ flat outputを確認します。この検証は
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

### side / thickness の追加検証

同じ Windows / Vulkan / RTX 5090 環境で side の実 GPU 10件を追加し、描画全回帰48件が dev profileで
通過しました。通常workspaceテスト903件（doctest 1件を含む）、Clippy全target、整形検証も通過しています。
新規side 10件は releaseでも通過しています。
静止・回転中のnative PNGを確認し、側面はtop近傍、影はその先に描かれることと、4 piece結合部に内部の
暗い線がないことを確認しました。GPU state / rotation recordとstorage bindingの追加はありません。

side追加後の同じ1000² grid / 512² offscreen / 白1×1不透明textureで、releaseの12 frame平均を
短い1 runとして記録しました。全qualityで両draw 0、追加state / root / rotation / mapping / delta /
selection / drag upload 0、両optional pipeline cache未生成、全pixel一致、可視instance数100万です。

| quality | visibility GPU (ms) | top draw GPU (ms) | shadow draws | side draws |
| --- | ---: | ---: | ---: | ---: |
| Low | 0.0250 | 0.5120 | 0 | 0 |
| Medium | 0.0261 | 0.4929 | 0 | 0 |
| High | 0.0263 | 0.5123 | 0 | 0 |

GPU timestampの短期変動を含む値であり、導入前との速度向上率・60 fps・別環境の互換性は示しません。

同日、半透明connected componentのfixtureを1件追加し、sideの実GPU 11件がdev profileで
通過しました。Clippy全target・整形検証も通過しています。上記のpixel center一致時の境界問題は
切り分けて記録し、描画コードは変更していません。

### top bevel の追加検証

同じ Windows / Vulkan / RTX 5090 環境でbevelの実GPU 8件を追加し、描画全回帰57件がdev profileで
通過しました。通常workspaceテスト905件（doctest 1件を含む）・Clippy全target・整形検証も通過しています。
Medium / Highの静止、Highの回転中と結合回転中のnative PNGを確認しました。
追加draw / pipeline variant / storage binding / per-piece stateはなく、uniformは336 bytesです。
非同期の初回shadow fixtureにはBevy最終出力pipelineの表示待ちを適用し、RenderReadyの検証後に
実pixelを確認します。表示済みepochのcompile直後frameを待たずに読む回帰は従来のままです。

新規bevel 8件はreleaseでも通過しました。既存100万piece俯瞰fixtureの同じ1000² grid / 512² target /
白1×1不透明textureでも、全qualityで`bevel_enabled == 0`、shadow / side draw 0、追加data upload 0、
両optional pipeline cache未生成、可視instance数100万、Lowとの全pixel一致を確認しています。
releaseの12 frame平均を短い1 runとして記録しました。

| quality | visibility GPU (ms) | top draw GPU (ms) | bevel | shadow / side draws |
| --- | ---: | ---: | --- | --- |
| Low | 0.0255 | 0.4793 | off | 0 / 0 |
| Medium | 0.0259 | 0.4935 | off | 0 / 0 |
| High | 0.0255 | 0.4940 | off | 0 / 0 |

短期timestamp変動を含む値です。bevel有効時のGPUコストや、導入前との速度比較・60 fps・別環境の
互換性を示す測定ではありません。
