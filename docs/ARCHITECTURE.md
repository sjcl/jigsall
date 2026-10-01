# Puzzella のアーキテクチャ

2026-10-01。基準`22e0aa135c5bdc6a881a3fe2ab6d976087d728ba`のnative lyon generator v2を参照として残し、procedural GPU rendererへ移行しました。現在はv4の楕円弧の付け根を保ちながら辺の識別性を高めたgenerator v5です。v3移行時の数値は[PROCEDURAL_RENDERER.md](PROCEDURAL_RENDERER.md)、付け根修正は[ROOT_TRANSITION.md](ROOT_TRANSITION.md)、現在のclass decodeと検証結果は[EDGE_FINGERPRINT.md](EDGE_FINGERPRINT.md)を参照してください。

## Workspaceと責務

```text
puzzella
  ├── puzzella-ui → puzzella-game / puzzella-puzzle
  └── puzzella-game → puzzella-core / puzzella-puzzle
                                         └── puzzella-core
```

共通の依存バージョン・Cargo.lock・targetをworkspaceで管理し、全packageをdefault-membersに含めています。

| ファイル | 責務 |
| --- | --- |
| `core/src/gameplay.rs` / `commands.rs` | row-major PieceId、PuzzleDefinition、CPU命令検証、snap |
| `puzzle/src/procedural.rs` | u32 hash、packed EdgeProfile、解析形状・UVのCPU参照 |
| `puzzle/src/fingerprint.rs` | feature / test限定のmacro fingerprint、輪郭descriptor、凍結v4測定参照 |
| `puzzle/src/placement.rs` / `grid.rs` | O(N)格子リング配置、seed付きshuffle、grid |
| `puzzle/src/shapes.rs` / `generation.rs` | feature / test限定のv2 Bezier・lyon・Rayon・U16 geometry |
| `game/src/resources/pieces.rs` | 16-byte dense正本、dense owner IDs、selection / dirty mask、bulk authority、drag bitset / delta、dirty upload |
| `game/src/interaction.rs` / `systems/piece_interaction.rs` | 非同期選択のgesture、命令発行、矩形overlay |
| `game/src/systems/game_logic.rs` | 命令適用、Release後のsnap、イベント駆動の進捗 |
| `game/src/systems/puzzle_generation.rs` | placement worker、GPU準備待ち、開始・失敗 |
| `game/src/render/mod.rs` | GPU buffers、Core2d pass、indirect draw、非同期readback |
| `game/src/render/puzzle_shape.wgsl` | main / point / rectangle共通の形状・UV |
| `game/src/render/puzzle_render.wgsl` | shader生成quad、画像・outline、ID / bitset出力 |
| `game/src/render/visibility.wgsl` / `pick_visibility.wgsl` | culling、selectable bitset、可視IDの安定圧縮、選択ROI |
| `game/src/render/radix_sort.wgsl` | visible countからindirect dispatch、24bit Zの安定radix sort |
| `game/src/selection/` | API、論理→物理座標、要求順序、readback復号 |
| `ui/` | egui設定・メニュー・HUD・進捗 |

通常依存からlyon、lyon_tessellation、Rayonを外しました。`cpu-geometry-reference`はv2参照を、`cpu-picking-debug`は加えてCPU triangle判定を有効にします。通常の選択はGPUです。

## CPU正本と入力

`PieceDataStore.states: DensePieceStates`が正本です。内部は固定長の`Arc<[GpuPieceState]>`で、`PieceId(n)`は`states[n]`を直接参照します。position、u32 z_order、flagsの16 bytesです。grid位置、正解位置、size、UV、辺パラメータ、boundsは定義とIDから導出します。全ピース分のPuzzlePieceやTransformは保存しません。確定選択とdirty IDは`PieceBitSet`、holderはdense PlayerIdとoccupancy maskです。矩形previewはGPU bitsetを直接outlineへ利用し、release時だけCPU maskへreadbackします。drag中の一時移動は固定membership bitsetとdeltaで表現し、最終座標だけをrelease時にCPU正本へ反映します。[MILLION_SELECTION.md](MILLION_SELECTION.md)に移行・計測・メモリを記載しています。

```text
mouse / Ctrl / rectangle / multi-drag
  → ClientCommand { player, PieceCommand }
  → PieceDataStore::apply_command（所有者・placed・有限座標・mask寸法の検証）
  → dense CPU state + owner / dirty mask
  → ReleaseGroupのdelta commit → 所有権解放 → 各pieceのsnap_piece
  → placed_count → O(1)進捗更新 → 完成
```

Moveの最終座標を適用してからReleaseとsnapを処理します。bulk grabとreleaseはそれぞれ1つのClientCommandで、pieceごとの完了・配置Messageも生成しません。snap閾値は`distance < snap_distance`。配置済みピースは再Grabできません。保持者の異なる命令と非有限座標を拒否します。

入力はPostUpdateのegui処理、camera pan / zoom / edge scrollingの後です。現Transformで座標変換し、UI上の押下を抑制します。開始済みdragはUIを横切っても継続・解放できます。pauseとfocus lossで保持を解放し、未確定の矩形選択を元に戻します。

`PieceInteraction`はIdle / Dragging / BoxSelectingを持ちます。point結果の受信前にreleaseした場合も最終座標を保持します。矩形previewとrelease時の確定要求を分け、古いGPU応答が確定選択を上書きしないようにします。

## Dirty同期とZ順序

Last scheduleで選択maskのArcを共有し、Render側はそのidentityが変わった場合だけmaskをuploadします。selected outlineはfragmentで専用bitsetを参照し、dense stateのflagsとdirty rangeを変更しません。初回state uploadはCPU正本と同じArcを共有し、stateをコピーしません。次のLast / ExtractScheduleで初回snapshotを解放した後、通常の編集は同じ領域を更新します。共有中の例外的な早期編集はcopy-on-writeでsnapshotを保護します。dirty bitsetのset bitsをID順にiterateして連続rangeへまとめ、ID Vecの展開・sortは不要です。ExtractScheduleはArcと小さな定義をcloneし、Render側がrangeをqueue.write_bufferします。idle frameのstate / selected / membership uploadは0 bytes、1ピース移動は16 bytesです。通常frameにCPUの全件走査はありません。

初期ZはID、next_zはpiece_count。Grabでnext_z++を割り当て、グループ内の順序を維持します。shaderは24-bit整数範囲のreverse-Zへ変換します。100万ピースでは約1577万回のfront操作まで再圧縮不要です。上限でのみ順序を保つO(N log N)のslow pathを実行します。

## 生成と状態遷移

```text
AppState: Menu → GameSetup → InGame → GameComplete
                                  └──────────→ Menu
GameSubState: Initializing → Playing ⇄ Paused
Generation: NotStarted → GeneratingState → UploadingGpu → Completed / Failed
```

背景workerは中央除外領域外の格子リングslotを最終dense state領域へ直接書き込み、ChaCha8でshuffleしてからID順のZを割り当てます。main worldはその領域の所有権を受け取り、position Vecや全stateの初回uploadコピーを作りません。100万件の生成領域は16,000,000 bytesとArc headerです。GPU storage limitとpipelineエラーは生成失敗として表示します。GPU bufferとmain pipelineの準備後にPlayingへ進みます。ピースごとのasset登録phaseはありません。

[CPU benchmark](../game/examples/initialization_bench.rs)と[CSV](../benchmarks/dense-initialization.csv)は4096²画像寸法・seed 42・releaseで各サイズ5回です。100万件の中央値はworker生成6.4783 ms、main側の所有権受け取り0.0024 ms、初回upload準備を含む`app.update` 0.0810 msでした。schedule overheadを含み、実GPU upload・GPU準備待ち・worker threadの起動時間は含みません。生成・受け取り・初回upload・共有解放後の編集で同じallocationを使うこともassertしています。論理allocationの削減であり、OS RSSのピークは未測定です。

画像workerはデコード結果を`into_rgba8`で消費し、既にRGBA8ならpixel領域を再利用します。画像は`RenderAssetUsages::RENDER_WORLD`を使い、Bevy 0.19.1のextractがpixel Vecをrender worldへ移します。GPU upload後にCPU pixelデータは保持しません。main worldにはImageの寸法などのmetadataとhandle、PuzzleImageのopaque判定を残し、背景Spriteとpieceが同じGPU textureを使います。4096² RGBA8画像のCPU常駐64 MiBとextract時の同サイズのcloneを削減します。

完成画面ではパズルを残します。Menuへ戻る際、定義・state・画像・背景・選択・gesture・overlay・worker受信器・命令を清掃します。epochでGPU stateを作り直し、request IDをセッション間で再使用せず、前セッションの遅延readbackを無効にします。

## GPU presentation

Core2d main transparent pass後のカスタムpassです。背景画像Spriteは通常Bevy描画。GPUは拡張quad AABBでvisible IDとindirect argsを生成し、mainはdraw_indirect1回です。4頂点はvertex_indexから作り、vertexで4辺を2 u32ずつ生成してflat varyingへ渡します。fragmentはSDF・画像alphaでdiscardし、UV・outlineを評価します。

opaqueは任意のinstance順でdepth test/write、半透明は可視IDだけをGPU radix sort（8bit × 3 pass）で後方→前方に並べblendし、depthを書きません。透明経路ではID順に可視IDを圧縮してから安定sortし、同じZのID順も維持します。workgroup数はGPUのinstance_countからindirect dispatchで決め、CPU readbackは不要です。matrix・state・visibleをpickingにも共有します。矩形overlayは追加draw1回です。sortは[TRANSPARENT_RADIX_SORT.md](TRANSPARENT_RADIX_SORT.md)、選択は[GPU_PICKING.md](GPU_PICKING.md)に記載しています。

## Multiplayerの境界と課題

PieceIdはEntity IDから独立したu32、PlayerIdはu64です。version、seed、grid、画像寸法で形状を再構成します。core/sessionはsession identity・画像hash・命令sequence・authority epoch・migrationを、game/multiplayerはsnapshotの検証・復元とplayer単位の保持解放を提供します。GrabGroup / ReleaseGroupも同じ認証済みplayerとreliable control streamを使います。selectionはlocal presentationでありsnapshotには入りません。transport・途中参加のbackend連携・ネットワーク向けレート制限は未実装です。

GPUは描画と選択の補助で、placed・所有権・snapを決めません。cullingはO(N)、半透明sortは可視数に比例します。選択保持・drag pointer・rectangle previewのCPU処理はO(1)ですが、final selectionの再検証、grab時のZ順保持、release時のsnapとstate commitには明示的な大量処理が残ります。極端な重なりではrasterとpickingの負荷が増えます。異OS/GPU、通常windowの全手動操作は今後の確認対象です。
