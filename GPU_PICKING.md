# GPU picking 実装報告

## 実装前の調査

Bevy 0.19.1 / bevy_egui 0.42。lyon の indexed Mesh が形状の正本で、Mesh2d と共有 ColorMaterial（元画像1枚、AlphaMode2d::Blend）で描画していた。通常はZ順の連続範囲を結合して描画し、選択中だけ元Meshの一時Entityへ戻す。独自material pipelineや全面的なinstancingはない。カメラはrootのMainCamera / Camera2dで、XY scaleがzoom。従来の選択はR-tree候補検索とCPUのtriangle点・矩形判定だった。

0.19.1のRenderGraphはScheduleになっているため、独立したgraph systemを統合点にした。APIはインストール済み0.19.1のソースと[RenderGraph](https://docs.rs/bevy/0.19.1/bevy/render/renderer/struct.RenderGraph.html)、[MeshAllocator](https://docs.rs/bevy/0.19.1/bevy/render/mesh/allocator/struct.MeshAllocator.html)を確認して実装した。

## 1. Architecture

`PuzzleSelectionPlugin`をGamePluginから登録。入力・ゲームロジック・ClientCommandの境界は維持し、通常のCPU pickingをGPU要求/結果へ置換した。R-treeや衝突用triangleコピーの生成・更新・debug systemsは`cpu-picking-debug` featureまたはテスト時だけ有効。featureを指定しても通常の選択経路はGPUのまま。

生成時に元Meshへ`ATTRIBUTE_PIECE_ID`（Uint32）を追加し、結合時にもその属性を保持する。MeshAllocatorの通常描画用vertex/index bufferを直接使用する。picking専用のCPU geometryやvertex/index bufferは作らない。Transformは通常描画EntityのGlobalTransform、画像とsamplerも通常描画のGpuImageを共有する。

全PieceIdには軽量な`PuzzlePieceId` Entityを1つ生成する。これは描画用の一時Entityと独立し、バッチ抽出・返却で変わらない。結果の`entities`はこのidentity Entityで、ゲームロジックは従来通り`piece_ids`を使用する。Entityの内部整数はGPUに渡さない。

## 2. Types / systems / node

- `SelectionRequest { request_id, region, mode }` / `SelectionMode::{Point, Rectangle}`
- `SelectionResult { request_id, mode, piece_ids, entities, error }`
- `PuzzleSelection`: request発行・取消・最新結果取得。IDはセッションを跨いで単調増加する。
- `PuzzlePieceId`: 安定IDとidentity Entityの対応。
- `extract_selection`: camera・対象cameraのVisibleEntities・Mesh/Transform/materialを共有抽出。
- `selection_node`（pass label: PuzzleSelectionNode）: pipeline選択、clear、rasterize、copyを記録。
- `map_results`: submit後に非同期mapを登録。
- `receive_results`: channelを非同期受信し、最新requestだけデコード・Entityへ対応付ける。

実装: `game/src/selection/`。要求APIは`api.rs`、座標変換は`coordinates.rs`、GPU描画・readbackは`render.rs`、shaderは`selection.wgsl`、テストは`render_tests.rs`。

## 3. GPU flow

```text
Main World: logical selection region + monotonic request ID
  → ExtractSchedule: visible rendered meshes / camera / material
  → physical scissor, buffers and cached pipeline
Rectangle: zero bitset → scissored fragments without depth → atomicOr
Point: physical pixel → crop projection → 1×1 R32Uint + reverse-Z depth → front-most ID
  → same command encoder copies bitset or ONE texel
  → submit → map_async → channel → latest-ID check
  → PieceId / identity Entity → existing selection / PieceCommand
```

矩形は各fragmentで`atomicOr(selection[id / 32], 1 << (id % 32))`。depth attachmentがなく、同じ画素を覆う全ピースが記録される。クリックは画面上の選択画素をcrop projectionで1×1のinteger targetへ投影し、GreaterEqualのreverse-Z depthによりGPUで最前面を決定する。バッチ内のZも元の頂点位置から反映される。CPUでクリック候補のZを比較する処理はない。bitsetのID 0は有効。整数targetだけ`id + 1`を格納し、0をnoneに使う。

## 4. Readback / lifecycle

矩形は最高PieceIdに対応するword数だけコピー。ID 0..9999なら313 words = **1,252 bytes**。クリックは**4 bytes**。full-screen ID textureのCPU転送はない。clear → render → copyは同じencoder内、mapはRenderSystems::Cleanupでsubmit後に登録する。poll/waitによる同期GPU待ちは追加していない。

bitset/stagingは最大3slotのpool。busy slotは再利用せず、slotが埋まった場合は最新要求を次フレームに再試行。容量不足のidle slotだけpower-of-twoへ拡張する。uniform buffer、pipeline cache、texture bind group、render targetsも再利用する。クリック用R32Uint / Depth32Floatは常に1×1で、通常画面の解像度変更時も再利用する。矩形用R8Unormだけ、矩形要求時に必要なphysical targetサイズで作成・更新する。クリックだけの操作では画面サイズのpicking attachmentを作成しない。

入力はPendingPoint / Dragging / BoxSelectingを区別する。GPU待ちの間のpress・移動・release座標を保持し、遅延結果でも最後のMove → Releaseを発行する。矩形previewは4 input framesごと、releaseでは必ず新しい最終要求を発行し、その結果だけで確定する。pause、focus loss、Menuへの清掃、新しいgestureで古い要求を無効化する。

## 5. Camera / coordinates

APIのregionはrender target左上を原点とする絶対logical座標。Camera::target_scaling_factorでphysicalへ変換し、四方向のドラッグをmin/maxで正規化する。矩形はfloor(min)/ceil(max)、クリックはfloor(position)の1画素。physical target boundsとphysical viewportで交差を取ってからscissorへ渡す。viewport offsetをカーソルから二重に引かない。

実際のcamera clip_from_viewとGlobalTransformの逆行列を使う。矩形は通常描画のviewportを使用する。クリックはphysical pixelをviewport相対座標へ変換し、その1画素だけをcrop行列でNDC全体に拡大する。viewportとscissorは原点(0,0)・サイズ1×1、readback元も(0,0)とする。cropはclip空間のX/Yだけを変更し、Z/Wを保持する。幅/高さゼロ、負座標、viewport外、NaN、zero targetは空結果となり、invalid scissorを渡さない。選択枠もlogicalの両端を現フレームのcameraでworldへ投影するため、pan/zoom中にGPU領域と同期する。

## 6. Overlap / transparency

矩形はdepthなし、クリックはdepthあり。対象cameraのvisibilityと通常描画のMesh/UVを共有する。UV transform・material alpha・texture/samplerを反映し、Opaqueはalphaを無視、MaskはColorMaterialと同じcutoffでdiscard、Blendはalpha == 0の画素をdiscardする。Blendの非ゼロalphaはクリック候補として扱う。透明な前面ピースの背後にあるピースはクリックできる。

パズルカメラをMsaa::Offにし、通常描画とpickingを単一サンプルで一致させた。輪郭にMSAAの追加coverageはない。枠線・選択UI・背景はピースのpicking passには含めない。

## 7. Performance

CPU転送は矩形面積に依存せずO(max PieceId / 8)。idle時に同じrequestを再submitしない。fragment invocationはscissor内に制限するが、vertex processingは可視batch全体に対して行う。通常描画のバッチ化をそのまま再利用してdraw数を抑える。約1万IDでもbitset容量は小さい。

クリック用attachmentは1×1 R32Uintと1×1 Depth32Floatで、format上は計8 bytes（実際のGPU割当・driver overheadを除く）。従来の4K ID/depth約63.3 MiBの画面サイズ依存を除去した。クリックのclearも1画素のみ。矩形用R8Unormは引き続きphysical targetサイズで保持し、4Kで約7.9 MiB、clearはattachment全体に及ぶ。大量ピースの60fpsや4Kの処理時間は測定していない。

## 8. Limitations

- 現在のゲームのMesh2d / ColorMaterial / indexed triangle / MainCamera 1台を対象とする。Sprite、custom shaderによる変形・discard、render layers以外の独自clip、多camera別の選択には追加実装が必要。
- MSAAを再度有効にする場合は整数targetと通常描画のsample coverageを再設計する必要がある。
- alpha blendの複数ピース合成に対し、最前面の非ゼロalphaをクリックする。合成への寄与の大きさによる選択は行わない。
- GPU能力としてfragment writable storage、integer render targetが必要。backendごとの全環境確認は未実施。
- 要求は抽出されたフレームのcamera/描画状態に対して実行する。GPU latencyによりpreview/確定に数フレームの遅延がある。
- IDが疎になると最高IDまでbitset容量が必要。通常のrow-major IDは連続している。GPU storage binding limit超過はerror resultを返す。
- 実GPU自動テストはoffscreen。実ウィンドウでの全操作・高DPIモニタの目視検証や1000+ピースの性能計測は追加確認が必要。

## 9. Tests / checks

```sh
cargo fmt --all --check
cargo check --workspace --locked
cargo clippy --workspace --locked --all-targets --all-features -- -D warnings
cargo test --workspace --locked
cargo test -p puzzella-game --locked gpu_raster_selection -- --ignored --nocapture
cargo build --locked
```

通常テストは既存22件に、四方向/DPI/viewport/invalid region、bitset境界/10,000 ID容量、stale callback、遅延クリックのrelease座標、最終矩形とin-flight previewの競合を追加。実GPUテストは明示実行するignored testとして含める。GPUテストはbasic / 1pixel / mesh空白 / multiple / occlusion / alpha BlendとMask / camera translationとzoom / viewport offset / 四方向 / batch ID保持 / resource reuseに加え、画面中央以外の1画素ピースと隣接画素の非hit、通常targetの4K resize後もclick targetが1×1で同じtextureを再利用すること、クリックだけでは矩形attachmentを作成・resizeしないことを検証する。GPU待ちの期限付きloopとsleepはテスト側だけに存在する。

## 10. Click extension point

クリックも今回の共通API・抽出・shader・poolへ実装済み。クリック用crop projectionと1×1 ID/depth targetを実装済み。同じvertex stage・alpha関数・非同期readbackを引き続き共有する。hover要求、複数cameraや異なるmaterialの対応はrequest APIとpipeline specializationを拡張する。networkingの変更は不要。

検証結果（2026-10-01 / Windows / Rust 1.97）: fmt、check、all-targets/all-features clippy（警告をエラー化）、build成功。通常27 tests passed / GPU test 1件は既定でignored。明示したGPUテストもNVIDIA GeForce RTX 5090で成功。
実行ファイルの8秒起動確認も成功し、初期化出力・プロセス継続・stderrが空であることを確認した。目視でのゲーム操作検証は未実施。

## 1×1 click target更新

座標変換のunit testでは4K・viewport offset・1×1 viewport・DPI 2.5を対象に、画素の両端がNDCの±1、中心が0、隣接画素の中心が範囲外に変換されること、およびhomogeneous Z/Wが保持されることを確認する。クリックAPI、4 bytesのreadback、矩形のbitset方式は従来通り。

今回の更新の検証結果: workspace通常テスト29件成功、実GPUテスト1件成功（NVIDIA GeForce RTX 5090）。fmt、workspace check、all-targets/all-features clippy（-D warnings）、build、diff checkも成功。
