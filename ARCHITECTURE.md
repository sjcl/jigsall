# Puzzella のアーキテクチャ

2026-10-01。基準`22e0aa135c5bdc6a881a3fe2ab6d976087d728ba`のnative lyon generator v2を参照として残し、通常runtimeをprocedural GPU generator v3へ移行しました。数値は[PROCEDURAL_RENDERER.md](PROCEDURAL_RENDERER.md)を参照してください。

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
| `puzzle/src/placement.rs` / `grid.rs` | O(N)格子リング配置、seed付きshuffle、grid |
| `puzzle/src/shapes.rs` / `generation.rs` | feature / test限定のv2 Bezier・lyon・Rayon・U16 geometry |
| `game/src/resources/pieces.rs` | 16-byte dense正本、sparse holder、選択集合、dirty upload |
| `game/src/interaction.rs` / `systems/piece_interaction.rs` | 非同期選択のgesture、命令発行、矩形overlay |
| `game/src/systems/game_logic.rs` | 命令適用、Release後のsnap、イベント駆動の進捗 |
| `game/src/systems/puzzle_generation.rs` | placement worker、GPU準備待ち、開始・失敗 |
| `game/src/render/mod.rs` | GPU buffers、Core2d pass、indirect draw、非同期readback |
| `game/src/render/puzzle_shape.wgsl` | main / point / rectangle共通の形状・UV |
| `game/src/render/puzzle_render.wgsl` | shader生成quad、画像・outline、ID / bitset出力 |
| `game/src/render/visibility.wgsl` / `pick_visibility.wgsl` | culling、selectable bitset、透明sort、選択ROI |
| `game/src/selection/` | API、論理→物理座標、要求順序、readback復号 |
| `ui/` | egui設定・メニュー・HUD・進捗 |

通常依存からlyon、lyon_tessellation、Rayonを外しました。`cpu-geometry-reference`はv2参照を、`cpu-picking-debug`は加えてCPU triangle判定を有効にします。通常の選択はGPUです。

## CPU正本と入力

`PieceDataStore.states: Vec<GpuPieceState>`が正本です。`PieceId(n)`は`states[n]`を直接参照します。position、u32 z_order、flagsの16 bytesです。grid位置、正解位置、size、UV、辺パラメータ、boundsは定義とIDから導出します。全ピース分のPuzzlePieceやTransformは保存しません。holderはsparse HashMap、選択・preview・dirty IDは集合です。

```text
mouse / Ctrl / rectangle / multi-drag
  → ClientCommand { player, PieceCommand }
  → apply_piece_command（所有者・placed・有限座標の検証）
  → dense CPU state + dirty ID
  → 受理したRelease後のPieceMoveCompleted
  → snap_piece（Definition + IDから一時的に定義を導出）
  → PiecePlacedEvent → placed_count → 完成
```

Moveの最終座標を適用してからReleaseとsnapを処理します。snap閾値は`distance < snap_distance`。配置済みピースは再Grabできません。保持者の異なる命令と非有限座標を拒否します。

入力はPostUpdateのegui処理、camera pan / zoom / edge scrollingの後です。現Transformで座標変換し、UI上の押下を抑制します。開始済みdragはUIを横切っても継続・解放できます。pauseとfocus lossで保持を解放し、未確定の矩形選択を元に戻します。

`PieceInteraction`はIdle / Dragging / BoxSelectingを持ちます。point結果の受信前にreleaseした場合も最終座標を保持します。矩形previewとrelease時の確定要求を分け、古いGPU応答が確定選択を上書きしないようにします。

## Dirty同期とZ順序

Last scheduleで選択に変更がある場合だけflagsを同期します。初期化時は全stateを一度Arcへコピーし、その後はdirty IDをsortして連続rangeへまとめます。ExtractScheduleはArcと小さな定義をcloneし、Render側がrangeをqueue.write_bufferします。idle frameのstate uploadは0 bytes、1ピース移動は16 bytesです。通常frameにCPUの全件走査はありません。

初期ZはID、next_zはpiece_count。Grabでnext_z++を割り当て、グループ内の順序を維持します。shaderは24-bit整数範囲のreverse-Zへ変換します。100万ピースでは約1577万回のfront操作まで再圧縮不要です。上限でのみ順序を保つO(N log N)のslow pathを実行します。

## 生成と状態遷移

```text
AppState: Menu → GameSetup → InGame → GameComplete
                                  └──────────→ Menu
GameSubState: Initializing → Playing ⇄ Paused
Generation: NotStarted → GeneratingState → UploadingGpu → Completed / Failed
```

背景workerは中央除外領域外に格子リングslotを生成し、ChaCha8でshuffleします。main worldが結果をdense state化します。GPU storage limitとpipelineエラーは生成失敗として表示します。GPU bufferとmain pipelineの準備後にPlayingへ進みます。ピースごとのasset登録phaseはありません。

完成画面ではパズルを残します。Menuへ戻る際、定義・state・画像・背景・選択・gesture・overlay・worker受信器・命令を清掃します。epochでGPU stateを作り直し、request IDをセッション間で再使用せず、前セッションの遅延readbackを無効にします。

## GPU presentation

Core2d main transparent pass後のカスタムpassです。背景画像Spriteは通常Bevy描画。GPUは拡張quad AABBでvisible IDとindirect argsを生成し、mainはdraw_indirect1回です。4頂点はvertex_indexから作り、vertexで4辺を2 u32ずつ生成してflat varyingへ渡します。fragmentはSDF・画像alphaでdiscardし、UV・outlineを評価します。

opaqueは任意のinstance順でdepth test/write、半透明はGPU bitonic sortで後方→前方に並べblendし、depthを書きません。matrix・state・visibleをpickingにも共有します。矩形overlayは追加draw1回です。詳細は[GPU_PICKING.md](GPU_PICKING.md)に記載しています。

## Multiplayerの境界と課題

PieceIdはEntity IDから独立したu32、PlayerIdはu64です。version、seed、grid、画像寸法で形状を再構成します。transport導入時はsession identity、画像hash、snapshot、認証済みplayer、命令sequenceが必要です。通信・途中参加・切断時の保持解放・ネットワーク向けレート制限は未実装です。

GPUは描画と選択の補助で、placed・所有権・snapを決めません。大規模な半透明画像ではGPU sortが最大の負荷です。大量選択では集合のメモリとCPU処理が増えます。異OS/GPU、通常windowの全手動操作、極端な重なり負荷は今後の確認対象です。
