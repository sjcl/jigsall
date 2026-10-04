# Procedural GPU picking

通常描画とpoint / rectangle pickingは、同じdense state、visible ID、画像texture、vertex関数を使います。通常zoomでは共通puzzle_shape.wgslのSDF / UV、far zoomではpixel中心へsnapしたsplatと代表色sampleを共有します。RenderMesh、Mesh attribute、ピースEntity、ATTRIBUTE_PIECE_IDは不要です。ゲーム状態の正本はCPUです。

## Coverageと候補

通常zoomではvertexがIDからquadと4辺のpacked profileを生成し、main / point / rectangleは共通sample_visibleでSDF外側とalphaゼロをdiscardします。outlineは内側に描くため、選択領域外へ広がりません。MainCameraはMsaa::Offです。

selectable bitsetはvisibility computeでflagsから生成し、placed・held・disabledを両選択から除きます。CPUで毎frame全件のbitsetを作りません。pointは最大Zの選択可能なピース、rectangleは範囲にfragmentを持つ全選択可能ピースを返します。後者は奥に隠れたピースも含む仕様です。

main visibleを候補源とし、選択時だけ追加computeでpoint画素または矩形のworld AABBへ絞り込みます。両方ともtabを含む保守的なquad boundsです。全100万ピースがvisibleでもクリック描画へ直接100万instanceを送りません。組み立てた100万ピースで候補16以下をassertしています。全件を同じ位置へ重ねる場合、この上限は成り立ちません。computeはGPUでO(N)、CPUは全件候補検索をしません。

## Far zoomのcoverage

隙間のある初期格子配置を全体表示すると、subpixel quadがpixel sampleを通らず0 fragmentになる場合があります。規則的な配置とsample位置の干渉による欠落を防ぐため、`extract_puzzle`が共通piece size、現在のworld view size、physical viewport sizeからO(1)でprojected sizeを計算します。短辺が`FAR_ZOOM_THRESHOLD_PX = 1.5`未満ならfar mode、以上なら従来のSDFです。最小footprintは`FAR_SPLAT_MIN_PX = 1.0`です。両定数は`game/src/render/mod.rs`で管理します。

far vertexはpiece中心をmain clipへ投影し、viewport offset込みのphysical pixel座標で`floor(center) + 0.5`へsnapします。各軸の大きさは`max(projected_piece_size, 1 px)`で、全visible IDを同じindirect drawで描きます。edge hash / packed profile生成、profile decode、tab / blank SDF、connected edge outlineを省略し、cell中央UVのLOD 0 sampleを使います。sample alphaが0ならmain / point / rectangleすべてdiscardし、半透明は既存のsort / blendを維持します。selectedは黄tint、previewは青tintで、黄が優先です。

opaqueのplaced splatが同一pixelへ重なる場合は、far modeだけreverse-Z rankを`1 + PieceId / count`にします。対応上限100万pieceで各IDを区別できる`[1, 2)`の範囲を使い、可視IDのatomic append順に関係なく最大IDの色が残ります。通常zoomのplacedは従来のrank 1、looseは従来の`z_order + 2`のままで、placedは最小rankのlooseより常に後ろです。opaque側へsortやdrawを追加せず、半透明の既存の安定ID順 / radix sortも維持します。

uniformはmain viewportのsize / origin、projected piece size、main 1 pixelのworld size、far flag、crop scale / offsetを16-byteの組にして追加します。pointではmain matrixとpixel scaleを保持し、snap済みquadへcropを最後に適用して1×1 targetへ写します。viewport sizeを1×1へ置き換えません。通常modeのpoint cropは従来のmatrix合成を維持し、rectangleはmain viewportとscissorをそのまま使います。

main / pick visibilityはfar時だけ`max(normal_half, splat_half) + 0.5 * main_pixel_world_size`へboundsを広げます。最小footprintに加え、snapで最大0.5 pixel移動する分も含めます。pick ROIでview boundsを更新してもpixel scaleはmainのままです。通常modeのcullingは従来どおりです。

`render/tests/far_zoom_tests.rs`は短辺・physical resolutionによる双方向threshold切替、0〜0.875 pxのX/Y位相、8段階のfractional pan、offset付き非正方viewport、point / rectangle coverage、縦横比のあるsplatと画面外中心、GPU drag、alpha 0 / 半透明、黄 / 青tintを検証します。integer pixel境界ではCPU / GPUの投影丸めにより隣のpixelへsnapする場合を許容し、各pieceが必ず1 pixelを持つことと、その実pixelでpickできることを確認します。100万pieceを`generate_placement_grid`の実初期配置に置くテストは、8 panすべてで全pixel coverage、indirect argsの100万instance、state 16 bytes / piece、camera frameのstate / root / selection / drag upload 0 bytes、全pieceのrectangle選択とpoint hitを確認します。

placed競合の回帰テストはcountを100万にし、ID 0 / 1 / 999998 / 999999を正解位置で同じpixelへsnapさせます。色の異なるopaque textureとテスト専用cullingで6種類の可視順を強制し、各順を複数frame繰り返して最大IDの色を確認します。自然なGPU append順が偶然安定していても検出でき、修正前は逆順でID 0の色が残って失敗します。最小Zのlooseを重ねるケースとplacedのpoint / rectangle選択除外も確認します。

2026-10-02、Windows / RTX 5090 / Vulkanで実GPU検証。far pathを一時的に無効化した対照実行では64個中3個しかcoverageを持たず、位相回帰テストが失敗しました。far pathでは全64個を維持します。main drawは1回、CPUの通常frameに全piece走査・専用far piece listを追加していません。

```sh
cargo test -p jigsall-game --release --locked far_zoom_threshold
cargo test -p jigsall-game --release --locked gpu_ -- --ignored --skip benchmark --nocapture --test-threads=1
```

## Point / rectangle

pointはcrop projectionで対象画素を1×1のR32Uint / Depth32Float targetへ投影し、PieceId + 1を出力します。0はno hitです。reverse-Zで手前を決め、4 bytesをcopy・非同期mapします。

rectangleはscissor内の同じgeometryからatomicOrでbitsetを設定します。depth testをしないので奥も返ります。readbackは4 * ceil(N / 32) bytes。1万は1,252 bytes、100万は125,000 bytesです。確保はpower-of-twoに丸めます。

矩形rasterはPieceId単位のdirect hit maskへ出力します。preview中だけ、そのwordのset bitを1回のGPU computeでcomponent rootのbitへcollapseします。main vertexがpieceごとにrootとcomponent preview maskを読み、結果のPREVIEW bitを既存のflat flagsでfragmentへ渡します。root専用varyingはありません。root / preview mask loadは最大4 vertices / pieceで、pixel数に比例しません。point / rectangle picking用uniformはpreview_activeを0にし、selection rasterのvertexはroot / preview maskを読みません。fragmentでcomponent rootへのatomicを集中させません。computeも同じmask wordへ向かうroot bitsをまとめてatomic ORします。CPU readback・CPU component展開・expanded mask uploadはpreviewに追加しません。

root mappingは4 bytes / pieceの専用GPU bufferです。CPU側の恒久root mirrorはなく、absorbed memberのdirty bitsetだけを持ちます。union-by-sizeのwinner / absorbedをlist splice前のcallbackで取得し、absorbed memberだけdirtyにします。upload時に最終DSU rootを求め、連続rangeへまとめます。同じRelease内の複数unionを重複なく反映し、128 spansを超える場合は既存state uploadと同じenclosing rangeへまとめます。epoch初回だけ現在のDSUから全rootを構築し、新規singletonとsnapshot restoreの両方に対応します。

direct hitは既存selectable maskでplaced / held / disabledを除外済みです。正規状態ではcomponent内のselectabilityが揃います。Grab / Release / placementはcomponent atomic、set_stateはsingleton限定、snapshot restoreはcomponentのplacement / translation整合性を検証し、全memberのENABLEDを復元してholdを解除します。このinvariantによりhitしたmemberのcomponent全体をpreviewでき、非selectable memberのroot走査や拒否maskは不要です。GPUは既存pickingと同じflags mirrorを参照し、CPU authorityを置き換えません。非同期authority変更との一時的なraceは最終CPU再検証で扱います。

previewとselectedは共通のselection_boundary_distanceで内部接続辺を除き、外周だけを青 / 黄で表示します。黄色が優先です。coverage / point / rectangle pickingのSDFは全辺のままです。各rectangle requestでdirect / preview maskをclearし、empty / clippedでも古いpreviewを残しません。取消はuniformで非表示にします。

release時のreadbackはcomponent maskではなくdirect hit maskの4 * ceil(N/32) bytesです。CPUのPieceBitSetへwordのまま復号し、commit_selection / selectable_members / canonical_membersによる最終展開・authority検証を維持します。確定selected bitsetだけを従来どおりuploadし、SELECTED flagのper-piece更新は不要です。

追加常駐GPUメモリはroot bufferの4N bytesとdirect maskの4 * ceil(N/32) bytes（1Mで合計4,125,000 bytes）です。既存preview bufferをcomponent maskとして再利用します。CPUの追加常駐metadataはdirty root bitset（1Mで125,000 bytes）で、初回root uploadだけ一時的な4N-byte Arcを持ちます。preview computeは1 dispatch、O(words + hits)で、root lookupはdirect hitのset bitだけです。idle / pointer dragはroot scan・root upload・preview computeを行わず、vertexの明示的なifによりpreview_active == 0ではrootを読みません。

## Multi-drag

Local未ACK Rotate / RotateDragはLast/upload準備でcanonical GpuPieceStateへ疎なpose
overrideを合成します。positionとrotation bitsだけが表示用になり、Z / HELD / ENABLED /
PLACED / connected-edge cacheは最新canonicalのままです。同じGPU state bufferをmain /
far / main visibility / pick visibility / point / rectangleが読むため、予測回転の位置・向きと
選択候補/coverageが一致します。退役/拒否した旧override rangeもcanonicalへrestoreします。
CPU正本・selection ownership・snapshotはpredictionを参照しません。通常pointer / camera /
pending ACK frameにはstate overrideの再計算/追加uploadがありません。

Direct-IPのremote dragもcanonical positionを変更せず、最大64 slotのpresentation deltaを適用します。`presentation.wgsl::presentation_position`をmain / pick visibilityと共通vertexが使用し、normal / far zoom / point / rectangleの位置計算を揃えます。remote-heldの選択除外は従来のcanonical HELDのままです。slot mappingのReliable境界、join / final reconciliation、GPU uploadとメモリは[ARCHITECTURE.md](ARCHITECTURE.md#remote-drag-presentation)を参照してください。

remote Transientのaccepted deltaはCPU presentationのtargetで、GPUの共有512-byte tableには`Time<Real>`で指数平滑化したdisplayed deltaを渡します。normal / far描画、main / point / rectangleのvisibilityとvertexはすべてこの同じ表示位置を使い、targetを直接参照するshader経路はありません。Reliable Grab / Ready / drag rotationは両deltaを即時一致させ、Release / Cancelは直ちにslotを破棄します。prediction / extrapolationは行いません。smoothing frameは最大64 slotだけを更新し、canonical / remote mapping uploadは0 bytes、delta uploadは最大512 bytes、exact settle後の次frameは0 bytesです。

2026-10-04、Windowsのrelease実GPUテストでsmoothingを追加検証しました。normal / farの途中display位置を描画し、target位置との区別とremote-heldのpoint / rectangle除外を確認しました。テスト時だけdelta bufferへCOPY_SRCを付け、3回のactive frameでGPUの値がCPU displayed deltaと一致することをreadbackしました。各frameのcanonical / remote mapping / local membership / root / selection uploadは0 bytes、remote deltaは512 bytes、exact settle後の次frameは0 bytesでした。通常runtimeのbuffer usageとshaderは変更していません。

```sh
cargo test --workspace --release --locked gpu_remote_presentation -- --ignored --nocapture --test-threads=1
```

2026-10-04、Windows / RTX 5090 / Vulkan（NVIDIA 610.88）でremote presentationを検証しました。canonical位置が画面外にある2つのheld pieceを異なるslot deltaで画面内へ移し、normal / farの描画とculling、point / rectangleのHELD除外を確認しました。scalar更新はcanonical state upload / remote mapping uploadが0 bytes、delta uniformが512 bytes、idleでは両remote uploadが0 bytesです。Release相当のmapping clearと同epochのsession reset後にoffsetが残らないことも確認しました。100万pieceのfar zoom回帰テストではremote mapping bufferが4,000,000 bytes、delta bufferが512 bytesで、camera / idleのremote uploadが0 bytesです。

```sh
cargo test -p jigsall-game --release --locked --all-features gpu_remote_presentation -- --ignored --nocapture --test-threads=1
```

開始時に選択可能な対象maskを固定し、1つのGrabGroupをauthorityへ渡します。authorityは所有権を再検証し、受理した対象の相対Z順を保ってGrabします。CPUのper-piece offset mapはありません。対象を示すbitsetを一度GPUへuploadし、移動中はworld-spaceのdrag_deltaだけをuniformへ渡します。main vertexとvisibility computeが同じ一時移動を適用するため、CPU正本が画面外にあるピースもdragで画面内へ入れます。heldの除外とZ順は維持します。

移動frameのCPU処理は選択数に対してO(1)、Move命令・state upload・membership uploadは0です。release時はmaskとdeltaを持つ1つのReleaseGroupで、位置のcommit・所有権解放・各pieceのsnap・placed_count更新まで処理します。pieceごとのcommand / Messageは不要です。pause / focus lossも最後に表示したdeltaを一度反映して解放します。確定選択・releaseは引き続きO(選択数)の処理を含み、Grabは相対Zを保つ一時sortも行います。

100万ピースのselectionとmembershipは各125,000 bytesのmaskです。GPUはselected / preview / membershipを別bufferに持ちます。gestureに対象ID Vecは保持せず、authorityのGrab時に相対Zのsort用一時Vecだけを使います。rollback snapshotはArc cloneで共有し、変更時だけ最大125 KBをcopy-on-writeします。PieceStateのpositionはdrag中に変わらず、一時移動はローカルpresentationのみです。2026-10-02の最新計測と制限は[MILLION_SELECTION.md](MILLION_SELECTION.md)を参照してください。

2026-10-01のWindows release CPU計測では、100,000回の`PieceInteraction::update`の平均は以下でした。gesture更新のみ（入力sampling・Bevy schedule・描画・Grab・releaseを除外）の計測で、frame timeではありません。

| 選択数 | 1回のgesture更新 |
| --- | --- |
| 1,000 | 13.142 ns |
| 10,000 | 13.833 ns |
| 100,000 | 13.744 ns |
| 1,000,000 | 14.585 ns |

## 座標・非同期要求

regionはcamera render target原点・左上基準の論理座標です。scale factorで物理座標へ変換し、viewport offsetとtarget boundsでclipします。pointは画素中心をcrop、rectangleは物理画素の半開区間をscissorにします。pan / zoomとviewport offsetを実GPUで確認しています。

readback slotは最大3個。busyなbufferを上書きせず、空きがない間は最新要求を後のframeへ回します。通常runtimeにGPU同期waitはありません。request IDは単調増加し、最新要求に一致する応答だけを受理します。cancel / cleanupでもIDをリセットしません。

gestureはpending point、preview、release時の最終rectangleを管理します。遅延結果、release前後、Ctrl toggle、pause / focus lossの取消を通常テストで検証しています。readback失敗はSelectionResult.errorへ渡します。

## 実GPU検証

```sh
cargo test -p jigsall-game --release --locked gpu_raster_selection -- --ignored --nocapture
cargo test -p jigsall-game --release --locked gpu_resized_texture_uses_logical_coordinates_and_shared_alpha_picking -- --ignored --nocapture --test-threads=1
cargo test -p jigsall-game --release --locked gpu_transparency_and_visibility -- --ignored --nocapture
cargo test -p jigsall-game --release --locked gpu_radix_sort_visible_counts_and_ties -- --ignored --nocapture
cargo test -p jigsall-game --release --locked gpu_drag_transform_and_preview_without_readback -- --ignored --nocapture
cargo test -p jigsall-game --locked gpu_component_rectangle_preview_matches_final_selection_without_readback -- --ignored --nocapture --test-threads=1
cargo test -p jigsall-game --locked gpu_component_preview_crosses_mask_words_and_preserves_direct_high_bits -- --ignored --nocapture --test-threads=1
cargo test -p jigsall-game --release --locked multi_drag_cpu_benchmark -- --ignored --nocapture
cargo test -p jigsall-game --release --locked procedural_gpu_benchmark -- --ignored --nocapture
```

2026-10-01、RTX 5090 / Vulkanで成功しました。Rust / WGSL raw hash、約9604画素のCPU shapeと実描画coverage、tab・neck・blank、Z順序、alpha blend、透明穴越しの選択、tabだけの可視性、camera移動、pan / zoom / viewport offset、16-byte単一uploadとidle 0-byte uploadを確認しました。

2026-10-04、RTX 5090 / Vulkan（NVIDIA 610.88）で縮小textureの追加検証に成功しました。logical sizeを128×128のまま、textureを32×32 / 16×16へ縮小し、色の表示、完全にalphaが0の穴、point / rectangle選択の一致を確認しました。Startupから取得したdeviceの最大辺は32768 px、DEVICE_LOCAL heapの報告容量は32187 MiBでした。Lanczos3の境界補間で小さいalphaが残る領域と、完全に透明な領域を区別したfixtureです。

移行時の計測は1k / 10k / 100k / 1Mのnear / medium / entireと全体半透明表示です。[CSV](../benchmarks/procedural-rtx5090.csv)と[計測条件](PROCEDURAL_RENDERER.md)を参照してください。現在のradix sortでは半透明のnear / medium / entireも計測し、[TRANSPARENT_RADIX_SORT.md](TRANSPARENT_RADIX_SORT.md)に記録しています。GPU完了waitは検証・計測fixture限定です。
