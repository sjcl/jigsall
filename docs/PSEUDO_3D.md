# 擬似3D描画

現在は hard shadow、厚紙の side / thickness、top surface の bevel / fake lighting を実装しています。責務と描画順序は
[アーキテクチャ](ARCHITECTURE.md#pseudo-3d-presentation)を参照してください。

## 品質と screen-space LOD

「設定 → グラフィック → グラフィッククオリティー」のLow / Medium / Highを「適用」で切り替えます。
既定はHighで、`settings.json`の既存`display.piece_visual_quality`へ保存します。fieldがない旧設定もHighです。
品質だけなら15秒の確認なしで即時更新・保存要求を行い、解像度 / 画面モードとの同時変更では
既存のpreview / Keep / Revertに含めます。設定の詳細は[設定ファイル](SETTINGS.md#グラフィッククオリティー)を参照してください。

`DisplaySettingsPlugin`はPostUpdateの設定action処理後に、`DisplaySettingsState.current`の品質を
`game/src/render/visuals.rs`の`PieceVisualQuality` resourceへ同期します。次の描画frameから
qualityを寸法ルール付きの`ResolvedPieceVisuals`へ変換し、extractで
`FramePieceVisuals`をO(1) resolveして既存の`PuzzleUniform`へ渡します。
renderer単独のfixtureは設定pluginを使わず、resourceを直接差し替えられます。
品質変更でpuzzle reload / GPU buffer再生成 / piece state変更 / metadata uploadを起こしません。
Auto、feature個別設定、frame-timeによる動的調整は未実装です。preset / LOD / activation方針は維持します。

以下の`p`は`piece_size_px.min_element()`、`clamp(scale × p, min, max)`の結果はphysical pxです。
per-piece / componentの大きさは参照せず、cameraから得た代表pieceのprojected短辺をframe全体で使います。

| shadow quality | projected 短辺の threshold | base offset | extra lift | opacity |
| --- | ---: | ---: | ---: | ---: |
| Low | 無効 | 0 px | 0 px | 0 |
| Medium | 14 px | clamp(0.030 p, 1.5, 4.5) | clamp(0.040 p, 2, 6) | 0.20 |
| High | 10 px | clamp(0.040 p, 2, 6) | clamp(0.060 p, 3, 9) | 0.25 |

| side quality | projected 短辺の threshold | thickness | neutral linear RGB | opacity |
| --- | ---: | ---: | ---: | ---: |
| Low | 無効 | 0 px | 0 | 1 |
| Medium | 22 px | clamp(0.016 p, 0.75, 2.5) | (0.10, 0.10, 0.10) | 1 |
| High | 14 px | clamp(0.025 p, 1, 4) | (0.06, 0.06, 0.06) | 1 |

| bevel quality | projected 短辺の threshold | width | highlight strength | shadow strength |
| --- | ---: | ---: | ---: | ---: |
| Low | 無効 | 0 px | 0 | 0 |
| Medium | 28 px | clamp(0.0125 p, 0.75, 2) | 0.06 | 0.09 |
| High | 18 px | clamp(0.020 p, 1, 3) | 0.10 | 0.14 |

offset は右下への正規化ベクトルに掛ける距離です。physical pixel 単位で、DPI による logical pixel とは
区別します。共通の `PSEUDO_3D_DIRECTION` は camera / piece の回転に影響されません。
Highの20 px pieceでは回転開始・中間・終了のshadow separationが2 → 5 → 2 px、sideは1 pxです。
100 px pieceでは4 → 10 → 4 px、sideは2.5 pxです。同じzoomならside / bevelはelevationで変わりません。
静止中も shadow を描きます。`elevation != thickness` であり、normalized elevation は追加 lift だけです。
side は静止中にも存在する別パラメータです。shadow と side の threshold は独立し、High の12 pxでは
shadowだけを描きます。Low / LOD off では optional pseudo-3D pass 自体を発行せず、pipeline の新規
queue / raster / fragmentもありません。threshold未満では寸法・opacity・lighting strengthも0にします。
bevel の LOD も独立し、High の16 pxでは shadow / side が有効でも bevel は無効です。
far zoom では threshold を下げても bevel を有効にしません。blur、実 Z elevation、selection lift は対象外です。

thresholdは従来のhard gateを維持します。寸法の最小値を小さく抑え、threshold以上では連続したclamp式を
使います。別のactivation rampは入れていません。bevelのhighlight / shadow strengthは固定preset値です。
`ProjectedDimension`がpresetの係数 / 最小 / 最大を保持し、`FramePieceVisuals`だけが解決済みpx値を持ちます。
最大cull offsetもframeのbase + full lift / side thicknessから求め、pickingのboundsには加えません。
quality / zoom変更は既存uniformだけを更新し、piece state / metadataのupload、buffer / bindingを追加しません。

| Highのprojected短辺 | side (px) | bevel (px) | shadow base (px) | lift extra (px) |
| --- | ---: | ---: | ---: | ---: |
| 20 px | 1 | 1 | 2 | 3 |
| 50 px | 1.25 | 1 | 2 | 3 |
| 100 px | 2.5 | 2 | 4 | 6 |
| 200 px | 4 | 3 | 6 | 9 |

## Drag elevation

local / remote の held membership に envelope を与えます。grab は0→1、release / cancel は
その時点の値→0を real time の80ms smoothstepで補間します。定数は
`game/src/resources/drag_elevation.rs` の `DRAG_ELEVATION_SECONDS` にまとめています。
短いgrab、fade途中の再grabも現在値から開始します。componentの全memberは同じrecordを参照します。
複数の独立componentを一度に再grabするときは、新しいmaskを旧presentation slotごとに分割します。
fade中のAが0.5、idleのBが0なら、それぞれ0.5→1、0→1へ進み、BをAの高さへ跳ね上げません。
slot 0と期限切れzero recordはidleとしてまとめ、DSU / canonical stateは参照しません。
rotationと独立に評価し、shadow vertexだけで `max(rotation_elevation, drag_elevation)` を使います。
Highのheld shadow separationはframeのbase + liftで5〜15 pxです。
side thickness・bevel幅/強度・top位置/Z/depth・pickingはelevationで変わりません。
interaction envelopeはqualityに依存せず、Low / LOD offでも進行します。shadow passの追加発行はありません。

`DragElevationPresentation` はmain-worldの描画専用resourceです。localのfrozen membershipのArcと、
既存remote cacheのmembership versionを監視します。remoteはaccepted grab / release / cancelと
teardownのmembershipを使い、Transient deltaだけではenvelopeを再開始しません。
releaseでtranslation membershipが消えても、liftのslot mappingとrecordをfade終了まで有効にします。
focus loss / disconnectなど同じepochのcleanupも下降し、新しいpuzzle epochではcacheを破棄します。
protocol・authority・save/snapshot・canonical state・`GpuPieceState`・connectivity/snapには追加しません。

member走査とslot mapping uploadはmembership変更/slot再利用の境界だけです。通常drag frameは時刻を
既存336-byte uniformに渡すだけで、piece state / member / record uploadはありません。
dirty IDは`PieceBitSet`で保持し、ID順のiteratorから連続upload rangeを作ります。100万pieceでも
dirty bitsetは125,000 bytesで、memberごとのtree nodeは作りません。rangeが128個を超える場合は
一つの包含rangeにまとめます。単一旧slotのgrabは元のmask Arcを共有し、混在時だけpartitionごとの
ID listを作ります。ID listの合計は新しいmaskのmember数で、slotごとの全piece bitsetは作りません。
GPUの既存piece metadata storage bindingを初回grab時だけ拡張し、4番目のSoA regionと16-byte envelope
recordのtailを置きます。pool拡張はGPU copyで既存metadataを保存します。storage binding上限8を維持します。
fade完了後のslotはzero recordにし、mappingの解除はslot再利用時まで遅らせます。
idleではuniform gateでlookup/補間をskipし、初回drag前のCPU/GPU mapping allocationも増やしません。
通常frameの追加draw・O(N) CPU処理・piece uploadはありません。

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
bevel band内だけ、現在fragmentから外向きへ `inside_px + 0.75 px` 進んだ位置で同じprofileの
`piece_edge_distances` → `outer_boundary_distance` を再評価し、正のdistanceになった境界だけを照らします。
移動は `dpdx(local)` / `dpdy(local)` でscreen-spaceからpiece-localへ変換するため、zoom / camera回転 /
piece回転 / continuous rotationでもprobeはphysical pixel基準です。凸タブ内部に残るnominal rectangleの
zero contourは、probe先も内部なので照明から除外します。形状・coverage用SDFとoutlineは変更しません。
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
cargo test --locked -p jigsall-game drag_elevation
cargo test --release --locked -p jigsall-game gpu_drag_elevation -- --ignored --nocapture --test-threads=1
cargo test --release --locked -p jigsall-game gpu_scaled_visuals -- --ignored --nocapture --test-threads=1
cargo test --release --locked -p jigsall-game --lib drag_elevation_million_grab_boundary_benchmark -- --ignored --nocapture --test-threads=1
cargo test --release --locked -p jigsall-game gpu_ -- --ignored --skip benchmark --nocapture --test-threads=1
```

`gpu_bevel_convex_tab_has_no_nominal_rectangle_seam` は単独の凸タブpieceをLowのflat referenceと比較し、
nominal rectangle edgeの両側とbevel幅内に明暗線がないこと、曲線・直線の実外周には明暗が残ることを
実GPUで確認します。0°の静止、90°、連続回転中間、zoom差、camera回転、High品質を含みます。
2026-10-07、Windows / RTX 5090 / Vulkan（NVIDIA 610.88）のreleaseで、
`cargo test -p jigsall-game --release --locked render::tests -- --include-ignored --skip benchmark --nocapture --test-threads=1`
を実行し、通常・実GPUのrendererテスト88件が成功しました。bevel、coverage、point / rectangle picking、
alpha hole、outline、side / shadow、buffer layoutとuploadの回帰を含み、性能benchmarkは対象外です。

`resources/drag_elevation/tests.rs` はgrab / hold / early cancel / release、fade中の再grab、部分解除、
remote slot再利用 / teardown / epoch切替、rotationとのmaxの連続性を検証します。
異なる高さのfade / idleを含む再grabと各partitionのrelease、断片化dirty rangeとslot再利用も確認します。
実際のlocal GrabGroup / ReleaseGroup / disconnect cleanupも観測し、canonical stateがpresentation更新で
変わらないことを確認します。100万memberで10,000回のpointer更新を行い、piece state accessと
mapping / record / range再生成がないことも確認します。
`drag_elevation_million_grab_boundary_benchmark` は100万memberの初回grab、期限切れslotの再利用、
50万memberがfade中・残りがidleのmixed re-grabを各7回測ります。fixture / maskの生成は計測外で、
presentationのmapping・dirty管理・partition・upload snapshot生成は計測内です。authorityのGrabGroup処理、
GPU buffer拡張 / 転送、入力から表示までの遅延は含めません。

`render/tests/scaled_visuals_tests.rs`は20 / 50 / 100 / 200 px、短辺の軸入れ替え、連続zoom、min/max clamp、
LOD / Low / far gateとpeak cull extentを検証します。実GPUでは実際のquality / camera scale変更を使い、
同じpx値を固定した基準と全pixel比較します。point / rectangle picking、metadata sizeと全data upload 0も
確認し、各zoomでrotation / dragのmaxをhost指定separationの基準画像と比較します。
`JIGSALL_VISUAL_PREVIEW_DIR`を指定すると、このfixtureは各quality / sizeの256² PNGを保存します。

`render/tests/drag_elevation_tests.rs` は開始・中間・held・release直後・終了のshadowを、同じposeで
hostが指定した分離距離の基準画像と全pixel比較します。connected全memberのslot共有、rotationとのmax、
opaque / alpha 0.5 / alpha 0でのside・bevel・point / rectangle picking不変、Low / LODのdraw/upload 0、
remote slot再利用とfadeを検証します。metadata拡張後もpreviewにroot領域だけをbindingし、padding bitが
optional領域をrootとして読まないことを実GPUで確認します。
mixed re-grab直前と直後の全pixel一致、A / Bそれぞれのshadow separation、中間からrelease終了までの
連続性も検証します。

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
far mode の1.5 pxより大きいため、far raster fixtureだけtest用threshold 0.25 pxと固定したoffsetを使います。
通常 production の far overview は shadow pass を完全 skip します。
shadow-only viewport fixtureも、保守的なquad paddingを越える固定separationを使ってcull拡張を検証します。
実際のclamp寸法と最大liftを覆うcull extentは`scaled_visuals_tests`で別途確認します。

`render/tests/side_tests.rs` は、静止 side、独立LOD、回転開始 / 中間 / 終了の一定幅、zoom / camera回転、
connected union の内部継ぎ目、normal / far の alpha、opaque depth / translucent sort、side-only picking、
viewport端を実 GPU で確認します。shiftした flat top の参照 silhouette と全 pixel を比較し、side が
elevation と無関係な同じ offset を使うことを確認します。side-only viewport fixture は cull 拡張を確実に
通る10 px厚、far fixtureは0.25 pxのtest-only thresholdと固定1.5 px厚を使います。
productionのclamp式とは独立したraster / clipping検証です。
非同期 fixture はLow → High、Highの12 px → 20 px、Medium → High、新 epoch初回Highを確認します。
shadow専用 fixture はsideを無効にして従来の個別保証を維持します。storage buffer 上限は同じ8です。

半透明 connected fixture は、alpha 128 の2×1 / 2×2 component を同じ外形・透明度・回転中心の
1枚の矩形pieceと比較します。High side の静止 / 回転開始 / 中間 / 終了、shadow無効 / 有効で、
全pixelの各channelが参照より余分に暗くならないことを確認します（8-bit出力の差1を許容）。
topの背後にsideが透ける全体的な暗さは参照にも含め、結合境界での追加の暗い帯を検出します。
1枚の参照pieceもmemberのprojected短辺から得た同じside / shadow寸法に固定し、外形全体の大きさで
厚みが変わることによる暗さをこの継ぎ目検証へ混ぜません。

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
曲線の狭いbevelはCPU中央差分ではなく2×2 pixel quadのfine / coarse derivative範囲で確認し、
両方で照明方向とbevel帯が確定するsampleだけを比較します。
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

### drag elevation の追加検証

2026-10-06、Windows / NVIDIA GeForce RTX 5090 / Vulkanで、通常workspaceテスト912件
（doctest 1件を含む）、全target Clippy、整形検証が通過しました。追加のCPU / layout回帰は7件、
実GPU回帰は6件です。既存を含む実GPU63件がdev profileで、追加6件がreleaseで通過しました。
grab前・held・release中間のnative PNGでもshadowだけのseparation変化を確認しました。
side・bevel・picking・canonical stateの不変、component共有、rotationとのmax、remote再利用と
disconnect fade、metadata拡張後のpreview padding除外を検証しています。

同じreleaseテスト実行ファイルで100万piece fixtureも通過しました。1000² grid / 512² offscreen /
白1×1不透明textureで、初回drag前のLow / Medium / Highはlift mapping未確保、shadow / side draw 0、
bevel off、追加data upload 0、optional pipeline cache未生成、全pixel一致、可視instance数100万です。
100万memberのheld中とrelease完了後も全pixelが基準と一致し、通常frameの全upload 0、
追加draw 0を確認しました。grab境界だけmappingと共通recordをuploadします。

releaseの12 frame平均を短い1 runとして記録しました。

| 状態 / quality | visibility GPU (ms) | top draw GPU (ms) | shadow / side draws |
| --- | ---: | ---: | --- |
| idle / Low | 0.0262 | 0.4876 | 0 / 0 |
| idle / Medium | 0.0259 | 0.4989 | 0 / 0 |
| idle / High | 0.0269 | 0.5335 | 0 / 0 |
| held / High | 0.0273 | 0.5067 | 0 / 0 |

短期timestamp変動を含みます。GPU速度の導入前比較・ゲーム全体の60 fps・別OSの実機互換性は
この検証から主張しません。

### mixed re-grab と grab 境界の修正

2026-10-06、同じ Windows / Vulkan / RTX 5090 環境で、CPU回帰3件と実GPU回帰1件を追加しました。
通常workspaceテスト915件（doctest 1件を含む）、全target Clippy、整形検証が通過しています。
描画全回帰64件はdev profileで通過しました。mixed re-grab直前と直後の全pixelが一致し、
fade中のAとidleのBが別々の開始値から持ち上がること、releaseも各現在値から下降することを確認します。
dirty IDの125,000-byte bitsetは初回grab時にだけ確保し、空のrelease / expiry更新はbitset走査も省きます。

最終コードのdrag実GPU7件と100万piece overview fixtureはreleaseでも通過しています。
overviewはheld中 / release完了後とも基準と全pixel一致し、追加draw / 通常frameのdata uploadは0です。
AMD Ryzen 9 9950X / Rust 1.97のreleaseで、100万memberのpresentation準備を各7回測定しました。
fixtureとmaskは計測前に生成し、初回のmapping確保やslot再利用時のmapping解除は計測に含めます。

| grab境界 | 中央値 (ms) | 最小 (ms) | 最大 (ms) |
| --- | ---: | ---: | ---: |
| 初回grab | 10.860 | 10.435 | 12.183 |
| 期限切れslot再利用 | 18.913 | 18.249 | 20.516 |
| 50万fade + 50万idleのmixed re-grab | 13.409 | 11.431 | 14.736 |

authorityの命令処理やGPU拡張 / 転送を含むgrab全体の遅延ではありません。変更前のBTreeMapとの
速度比較は行っていません。通常pointer frameのO(1)処理とupload不変は、別の100万member / 10,000更新の
回帰テストで確認します。

### projected piece size に応じた寸法の追加検証

2026-10-06、Windows / Vulkan / NVIDIA GeForce RTX 5090で、CPU回帰4件と実GPU回帰2件を追加しました。
最新masterを含む通常workspaceテスト921件（doctest 1件を含む）、全target Clippy、整形検証が
通過しています。描画全回帰66件はdev profileで通過しました。

High / Medium / Lowの20 / 50 / 100 / 200 pxを、同じ寸法を固定した参照画像と全pixel比較し、
quality / zoom変更のpiece state / metadata / 全data upload 0、metadata buffer寸法の不変、
point / rectangle pickingの一致を確認しました。各zoomのrotation + grab / releaseも、
host指定のmax elevationから得たshadow separationと全pixelが一致します。
uniformは336 bytes、追加buffer / binding / drawはありません。

全quality / 4サイズのnative 256² PNGを保存し、Highの20 / 50 / 200 pxとMediumの200 pxを
目視確認しました。寸法の強さはclamp式で変わり、bevelのstrengthとdrag / rotationのenvelopeは維持します。
曲線bevelと静止sideの既存fixtureは、狭いbandのquad derivativeとpixel位相に合わせて参照条件を更新しました。

最終コードの追加実GPU2件はreleaseでも通過しました。同じrelease実行ファイルの100万piece fixtureは、
1000² grid / 512² offscreen / 白1×1不透明textureで、Low / Medium / Highともshadow / side draw 0、
bevel off、optional pipeline cache未生成、全data upload 0、Lowとの全pixel一致を確認しています。
100万memberのheld中とrelease完了後も全pixelが基準と一致し、通常frameのupload / 追加drawは0です。
初回drag前のlift mappingは未確保で、quality / zoom解決によるmetadata拡張もありません。

releaseの12 frame平均を短い1 runとして記録しました。

| 状態 / quality | visibility GPU (ms) | top draw GPU (ms) | shadow / side draws |
| --- | ---: | ---: | --- |
| idle / Low | 0.0256 | 0.4944 | 0 / 0 |
| idle / Medium | 0.0250 | 0.4900 | 0 / 0 |
| idle / High | 0.0266 | 0.5258 | 0 / 0 |
| held / High | 0.0250 | 0.4877 | 0 / 0 |

短期timestamp変動を含む値です。導入前との速度比較・擬似3D有効時のGPU速度・ゲーム全体の60 fps・
別OSの実機互換性を示す測定ではありません。

### グラフィック設定UIへの接続検証

2026-10-06、設定のCPU回帰3件、実際のegui widgetの回帰1件、実GPU回帰1件を追加しました。
通常workspaceテスト925件（doctest 1件を含む）、全target Clippy、整形検証が通過しています。
旧displayでfieldがない場合のHigh、3品質のJSON round-tripと再起動時の読み込み、品質だけの
即時保存、display previewのRevert / Dismiss / timeout / Keepとrenderer resourceの同期を確認しました。
UIのLow / Medium / High選択・適用と翻訳catalog契約を検証し、小画面・両言語の既存表示回帰も通過しました。

Windows / Vulkan / NVIDIA GeForce RTX 5090で、追加1件と関連する実GPU4件がdev profileで通過しました。
設定actionの次のextracted frameから品質が一致し、epoch・canonical piece state・state / metadata /
visible / indirect args / selection / dragのbuffer IDを維持します。品質変更frameの全data uploadは0です。
Lowへ戻した全pixelも元のflat frameと一致します。renderer単独のresource直接変更と、表示済みepochでの
optional pipeline lazy compilationも通過しています。preset・threshold・elevation envelopeは変更していません。
