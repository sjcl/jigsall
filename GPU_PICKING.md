# Procedural GPU picking

通常描画とpoint / rectangle pickingは、同じdense state、visible ID、画像texture、vertex関数、共通puzzle_shape.wgslのSDF / UVを使います。RenderMesh、Mesh attribute、ピースEntity、ATTRIBUTE_PIECE_IDは不要です。ゲーム状態の正本はCPUです。

## Coverageと候補

vertexがIDからquadと4辺のpacked profileを生成し、main / point / rectangleは共通sample_visibleでSDF外側とalphaゼロをdiscardします。outlineは内側に描くため、選択領域外へ広がりません。MainCameraはMsaa::Offです。

selectable bitsetはvisibility computeでflagsから生成し、placed・held・disabledを両選択から除きます。CPUで毎frame全件のbitsetを作りません。pointは最大Zの選択可能なピース、rectangleは範囲にfragmentを持つ全選択可能ピースを返します。後者は奥に隠れたピースも含む仕様です。

main visibleを候補源とし、選択時だけ追加computeでpoint画素または矩形のworld AABBへ絞り込みます。両方ともtabを含む保守的なquad boundsです。全100万ピースがvisibleでもクリック描画へ直接100万instanceを送りません。組み立てた100万ピースで候補16以下をassertしています。全件を同じ位置へ重ねる場合、この上限は成り立ちません。computeはGPUでO(N)、CPUは全件候補検索をしません。

## Point / rectangle

pointはcrop projectionで対象画素を1×1のR32Uint / Depth32Float targetへ投影し、PieceId + 1を出力します。0はno hitです。reverse-Zで手前を決め、4 bytesをcopy・非同期mapします。

rectangleはscissor内の同じgeometryからatomicOrでbitsetを設定します。depth testをしないので奥も返ります。readbackは4 * ceil(N / 32) bytes。1万は1,252 bytes、100万は125,000 bytesです。確保はpower-of-twoに丸めます。

矩形previewは専用GPU bitsetへ直接出力し、main fragmentがそのbitsetから青いoutlineを描きます。preview要求はstaging buffer・copy・map・CPUのID集合・flags uploadを使いません。release時の最終矩形だけをreadbackし、CPUの確定選択へ反映します。preview描画passはmain passより前に実行し、取消時はuniformで非表示、空矩形はbitsetをclearします。

## Multi-drag

開始時に選択可能な対象IDを固定し、相対Z順にGrabします。CPUのper-piece offset mapはありません。対象を示すbitsetを一度GPUへuploadし、移動中はworld-spaceのdrag_deltaだけをuniformへ渡します。main vertexとvisibility computeが同じ一時移動を適用するため、CPU正本が画面外にあるピースもdragで画面内へ入れます。heldの除外とZ順は維持します。

移動frameのCPU処理は選択数に対してO(1)、Move命令・state upload・membership uploadは0です。release時だけ全対象の最終座標をCPU命令として反映し、Release後に通常のsnapを行います。pause / focus lossも最後に表示したdeltaを一度反映して解放します。Grab・確定選択・releaseは引き続きO(選択数)の処理を含みます。

100万ピースのmembershipはCPU/GPUそれぞれ125,000 bytes、GPU previewは追加125,000 bytesです。CPUには対象IDのVec（4 bytes/対象）が残ります。PieceStateのpositionはdrag中に変わらず、一時移動はローカルpresentationのみです。

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
cargo test -p puzzella-game --release --locked gpu_raster_selection -- --ignored --nocapture
cargo test -p puzzella-game --release --locked gpu_transparency_and_visibility -- --ignored --nocapture
cargo test -p puzzella-game --release --locked gpu_drag_transform_and_preview_without_readback -- --ignored --nocapture
cargo test -p puzzella-game --release --locked multi_drag_cpu_benchmark -- --ignored --nocapture
cargo test -p puzzella-game --release --locked procedural_gpu_benchmark -- --ignored --nocapture
```

2026-10-01、RTX 5090 / Vulkanで成功しました。Rust / WGSL raw hash、約9604画素のCPU shapeと実描画coverage、tab・neck・blank、Z順序、alpha blend、透明穴越しの選択、tabだけの可視性、camera移動、pan / zoom / viewport offset、16-byte単一uploadとidle 0-byte uploadを確認しました。

計測は1k / 10k / 100k / 1Mのnear / medium / entireと全体半透明表示です。[CSV](benchmarks/procedural-rtx5090.csv)と[計測条件](PROCEDURAL_RENDERER.md)を参照してください。GPU完了waitは検証・計測fixture限定です。
