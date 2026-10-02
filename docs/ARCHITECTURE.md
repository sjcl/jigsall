# Puzzella のアーキテクチャ

2026-10-01。基準`22e0aa135c5bdc6a881a3fe2ab6d976087d728ba`のnative lyon generator v2を参照として残し、procedural GPU rendererへ移行しました。現在はv4の楕円弧の付け根を保ちながら辺の識別性を高めたgenerator v5です。v3移行時の数値は[PROCEDURAL_RENDERER.md](PROCEDURAL_RENDERER.md)、付け根修正は[ROOT_TRANSITION.md](ROOT_TRANSITION.md)、現在のclass decodeと検証結果は[EDGE_FINGERPRINT.md](EDGE_FINGERPRINT.md)を参照してください。

## Workspaceと責務

```text
puzzella
  ├── puzzella-ui → puzzella-game / puzzella-puzzle / puzzella-core
  └── puzzella-game → puzzella-core / puzzella-puzzle
                                         └── puzzella-core
```

共通の依存バージョン・Cargo.lock・targetをworkspaceで管理し、全packageをdefault-membersに含めています。

| ファイル | 責務 |
| --- | --- |
| `core/src/gameplay.rs` / `commands.rs` | row-major PieceId、PuzzleDefinition、CPU命令検証、snap |
| `core/src/connectivity.rs` / `snapping.rs` | DSUと循環member list、正しいgrid隣接、translation candidateの決定 |
| `puzzle/src/procedural.rs` | u32 hash、packed EdgeProfile、解析形状・UVのCPU参照 |
| `puzzle/src/fingerprint.rs` | feature / test限定のmacro fingerprint、輪郭descriptor、凍結v4測定参照 |
| `puzzle/src/placement.rs` / `grid.rs` | O(N)格子リング配置、seed付きshuffle、grid |
| `puzzle/src/shapes.rs` / `generation.rs` | feature / test限定のv2 Bezier・lyon・Rayon・U16 geometry |
| `game/src/resources/pieces.rs` | 16-byte dense正本、dense owner IDs、selection / dirty mask、bulk authority、drag bitset / delta、dirty upload |
| `game/src/resources/pieces/snapping.rs` | Release単位の単一snap判定、固定offsetのunion closure・一括配置 |
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
| `game/src/checkpoint.rs` | multiplayer / persistent 共通 capture・validation・DSU 復元・install |
| `game/src/persistence/` | versioned binary codec、画像 content addressing、backend 非依存 repository / logical storage、I/O worker |

通常依存からlyon、lyon_tessellation、Rayonを外しました。`cpu-geometry-reference`はv2参照を、`cpu-picking-debug`は加えてCPU triangle判定を有効にします。通常の選択はGPUです。

## CPU正本と入力

`PieceDataStore.states: DensePieceStates`が正本です。内部は固定長の`Arc<[GpuPieceState]>`で、`PieceId(n)`は`states[n]`を直接参照します。position、u32 z_order、flagsの16 bytesです。grid位置、正解位置、size、UV、辺パラメータ、boundsは定義とIDから導出します。全ピース分のPuzzlePieceやTransformは保存しません。確定選択とdirty IDは`PieceBitSet`、holderはdense PlayerIdとoccupancy maskです。矩形previewはGPU bitsetを直接outlineへ利用し、release時だけCPU maskへreadbackします。drag中の一時移動は固定membership bitsetとdeltaで表現し、最終座標だけをrelease時にCPU正本へ反映します。[MILLION_SELECTION.md](MILLION_SELECTION.md)に移行・計測・メモリを記載しています。

```text
GPU pick / selection mask
  → connectivity expansion（point / Ctrl / final rectangle）
  → GrabGroup 1個
  → authorityで再展開 → componentごとのownership検証・相対Z保持
  → drag bitset + delta（pointerはO(1)）
  → ReleaseGroup 1個
  → authorityのholdからcomponent全体を再検証 → 各componentへdeltaを1回commit
  → component snap resolver
      ├─ 正しいgrid隣接componentとのunion・連鎖結合
      └─ component全体のboard placement
  → dense state / dirty mask / placed_count → O(1)進捗更新
```

Moveの最終座標を適用してからReleaseとsnapを処理します。bulk grabとreleaseはそれぞれ1つのClientCommandで、pieceごとの完了・配置Messageも生成しません。連結componentは選択・ownership・移動・配置の単位です。scalar Grab / Move / Releaseもcomponent全体に適用し、同じresolverを使います。snap閾値はstrict `distance < snap_distance`。配置済みcomponentは再Grabできません。保持者の異なる命令と非有限座標を拒否します。

永続連結は`PieceConnectivity`の`Vec<i32> parent_or_size`と`Vec<u32> next_member`で表現します。union-by-sizeとpath compressionを使い、循環listのsuccessor交換でmember listをO(1)結合します。余剰bitsに最小member IDを保存し、offsetの代表とRelease処理順をsnapshot復元前後で揃えます。100万ピースで追加8,000,000 bytes、componentごとのEntity / 恒久member Vecはありません。隣接はrow-major IDから上下左右だけを導出します。Release直後のoffsetがstrict threshold内ならboardを優先しZEROへ配置します。範囲外の場合だけ、正しい隣接componentから最小offset距離、tieなら最小member PieceIdの順にtargetを1つ選びます。moving componentを一度だけ正規化し、以後のoffsetは固定します。固定final offsetへの再構成をmatches_translationで検証し、f32の算術丸めだけを許容した同一translationのvalid・unheldな隣接componentをclosureへ加えます。targetの座標は動かしません。Release共通scratchは少数IDをstackへ保存し、容量を超えたsetだけdenseへ昇格します。target検証と解決済みrootのlogical offsetをcacheし、成長するcomponentの再走査を抑えます。scalar Releaseはrootを直接処理し、Group Releaseも全componentのauthority検証後にaccepted maskを再構築せずmemberを処理します。詳細・計算量・計測・制限は[CONNECTED_SNAPPING.md](CONNECTED_SNAPPING.md)を参照してください。

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
GameCompleteSubState: Summary → Viewing ⇄ Paused
Generation: NotStarted → GeneratingState → UploadingGpu → Completed / Failed
```

背景workerは中央除外領域外の格子リングslotを最終dense state領域へ直接書き込み、ChaCha8でshuffleしてからID順のZを割り当てます。main worldはその領域の所有権を受け取り、position Vecや全stateの初回uploadコピーを作りません。100万件の生成領域は16,000,000 bytesとArc headerです。GPU storage limitとpipelineエラーは生成失敗として表示します。GPU bufferとmain pipelineの準備後にPlayingへ進みます。ピースごとのasset登録phaseはありません。

[CPU benchmark](../game/examples/initialization_bench.rs)と[CSV](../benchmarks/dense-initialization.csv)は4096²画像寸法・seed 42・releaseで各サイズ5回です。100万件の中央値はworker生成6.4783 ms、main側の所有権受け取り0.0024 ms、初回upload準備を含む`app.update` 0.0810 msでした。schedule overheadを含み、実GPU upload・GPU準備待ち・worker threadの起動時間は含みません。生成・受け取り・初回upload・共有解放後の編集で同じallocationを使うこともassertしています。論理allocationの削減であり、OS RSSのピークは未測定です。

画像workerはデコード結果を`into_rgba8`で消費し、既にRGBA8ならpixel領域を再利用します。画像は`RenderAssetUsages::RENDER_WORLD`を使い、Bevy 0.19.1のextractがpixel Vecをrender worldへ移します。GPU upload後にCPU pixelデータは保持しません。main worldにはImageの寸法などのmetadataとhandle、PuzzleImageのopaque判定を残し、背景Spriteとpieceが同じGPU textureを使います。4096² RGBA8画像のCPU常駐64 MiBとextract時の同サイズのcloneを削減します。

永続連結の追加後もdense stateの受け取り・初回uploadはcopy不要ですが、`initialize_dense`は新しい8-byte / pieceのDSU領域をO(N)で初期化します。上記0.0024 msはDSU導入前の受け取り測定で、現在の初期化コストは[CONNECTED_SNAPPING.md](CONNECTED_SNAPPING.md)のmetadata計測を参照してください。通常idle / pointerにこの処理はありません。

完成画面ではパズルを残します。GameCompleteのsubstateで結果カード・完成盤面の閲覧・ESCメニューを切り替え、InGameへ再入場せず定義・画像・配置を保持します。完成時に一度だけカメラを中央へ合わせ、閲覧中はpan / zoomを有効にします。Menuへ戻る際、定義・state・画像・背景・選択・gesture・overlay・worker受信器・命令を清掃します。epochでGPU stateを作り直し、request IDをセッション間で再使用せず、前セッションの遅延readbackを無効にします。

## GPU presentation

接続componentのselection / preview outlineは、dense stateのflags bit 5–8にあるtop / right / bottom / leftの接続cacheを使って内部辺を除外します。cacheはDSUの派生情報で、既存snap closureのneighbor探索内で両側をincrementalに更新し、変化したpieceだけdirtyにします。16-byte stateを維持し、snapshot schema 3のinstallでは復元DSUからcacheを再構成します。fragmentは4辺SDFを一度だけ計算し、coverage / pickingは全辺、黄 / 青outlineは共通の未接続境界を使います。全4辺が接続した内部pieceにoutlineはありません。

rectangleはselectableなdirect hitだけをmaskへrasterし、preview中だけ1回のGPU computeでcomponent rootのmaskへcollapseします。component atomicなauthority更新とvalidated restoreにより、正規状態のselectabilityはcomponent内で揃います。main vertexがpreview中だけrootを読み、flat varyingでfragmentへ渡します。final readbackは従来のdirect hit bitsetで、CPUのcommit_selectionがcomponent全体を再検証・確定します。GPU root bufferは4 bytes / piece、CPUにはroot dirty bitsetだけを持ち、unionでabsorbed memberをdirtyにして最終rootをrange uploadします。initial / restore時だけDSUから全rootを生成します。idle / camera / pointer dragでroot scan・root upload・preview computeはなく、preview_active == 0ならvertexもrootを参照しません。pipelineとメモリ・計算量は[GPU_PICKING.md](GPU_PICKING.md)に記載しています。

Core2d main transparent pass後のカスタムpassです。背景画像Spriteは通常Bevy描画。GPUは拡張quad AABBでvisible IDとindirect argsを生成し、mainはdraw_indirect1回です。4頂点はvertex_indexから作り、vertexで4辺を2 u32ずつ生成してflat varyingへ渡します。fragmentはSDF・画像alphaでdiscardし、UV・outlineを評価します。

opaqueは任意のinstance順でdepth test/write、半透明は可視IDだけをGPU radix sort（8bit × 3 pass）で後方→前方に並べblendし、depthを書きません。透明経路ではID順に可視IDを圧縮してから安定sortし、同じZのID順も維持します。workgroup数はGPUのinstance_countからindirect dispatchで決め、CPU readbackは不要です。matrix・state・visibleをpickingにも共有します。矩形overlayは追加draw1回です。sortは[TRANSPARENT_RADIX_SORT.md](TRANSPARENT_RADIX_SORT.md)、選択は[GPU_PICKING.md](GPU_PICKING.md)に記載しています。

## Persistent save の境界

ローカル進捗保存は `PieceDataStore / PuzzleDefinition → PuzzleCheckpoint → PuzzleSave → SaveCodec → SaveRepository → SaveStorage → FilesystemStorage` の流れです。画像は選択時の original encoded bytes を SHA-256 で識別し、再エンコードしない `.puzimg` を save 間で共有します。進捗 `.puzsave` v1 は 16 bytes/piece の明示的 little-endian codec と全体 / header checksum を使い、ランダム SaveId で保存します。最大492 bytesの header とファイル長だけで一覧を作り、非 authority の placed_count cache は完全 load 時に state と照合します。未 release の試作 format の互換コードは持たず、対応 version は1だけです。更新 request は読み込んだ revision を保持し、現在の header と不一致なら Conflict として publish 前に拒否します。ユーザータイトルは validation を持つ metadata で、filename / identity には使いません。

`GameSnapshot` schema 3 の serialized representation は保持し、borrowed checkpoint view を通じて同じ capture / validation / install を使います。restore は DSU / 接続 GPU cache / placed_count を再構築し、GameData の progress / completion を同期します。load worker が準備した store を直接採用するため、random 初期配置は生成しません。既存 epoch / RenderReady による GPU 準備待ちの後だけ Playing / GameComplete へ遷移します。disk I/O、decode、codec は worker/channel に分離し、O(N) capture は明示 Save 時だけです。通常 play に新しい piece 数比例の処理や per-piece Entity / persistent Vec は追加しません。

保存先は OS user application data 以下で、logical key を storage に渡します。画像を先に保存し、save は temporary file の sync と atomic replace で publish します。通常 Save は共有画像の存在だけを確認し、import / load で画像全体の hash を検証します。SaveStorage は Send / Sync を要求せず、filesystem は worker で動かします。将来の Steam Cloud は handle を所有 thread に保持し、StorageRequests の operation を非同期 API に dispatch、callback から返信する executor を追加します。StorageProxy を使う repository / codec / restore 準備は worker 上で継続します。write は encoded Vec の所有権を移譲し、proxy による全 blob コピーを避けます。Steam Cloud 自体は未実装です。形式・layout・failure / worker lifecycle の詳細は [PERSISTENCE.md](PERSISTENCE.md) を参照してください。

## Multiplayerの境界と課題

PieceIdはEntity IDから独立したu32、PlayerIdはu64です。version、seed、grid、画像寸法で形状を再構成します。core/sessionはsession identity・画像hash・命令sequence・authority epoch・migrationを、game/multiplayerはsnapshotの検証・復元とplayer単位の保持解放を提供します。GrabGroup / ReleaseGroupも同じ認証済みplayerとreliable control streamを使います。selectionはlocal presentationでありsnapshotには入りません。transport・途中参加のbackend連携・ネットワーク向けレート制限は未実装です。

transport向けにはcore/protocolのComponentRef / PieceTarget / ProtocolPieceCommandを使用します。minimum memberとexpected sizeでcomponentを参照し、32 componentまでcompact、より多いselectionはDenseへ切り替えます。game/multiplayer/protocolのopt-in authority adapterがcurrent connectivity・所有権・placed・enabledを再検証し、Grabで受理したcomponentだけをplayer別contextへ保持します。既存Move sequenceはmembership不要のbest-effort DragUpdateにも共用し、ReleaseはGrab sequenceとfinal deltaだけで確定します。local PieceCommand / PieceBitSetとGPU経路は維持します。詳細は[MULTIPLAYER_PROTOCOL.md](MULTIPLAYER_PROTOCOL.md)を参照してください。

snapshot schema 3は16-byte stateのflagsへPLACED / CONNECTED_RIGHT / CONNECTED_DOWNを保存します。root IDはprotocolへ保存せず、install時に隣接edgeからDSUを再構成します。schema 1 / 2、境界外edge、placedの誤座標、component内のoffset / placed不一致は変更前に拒否します。restoreはpositions / Z / connectivity / placedを保ち、holds / selection / dragをresetします。disconnectはcomponent全体のholdだけを解放し、位置とsnapを変更しません。

GPUは描画と選択の補助で、placed・所有権・snapを決めません。cullingはO(N)、半透明sortは可視数に比例します。選択保持・drag pointer・rectangle previewのCPU処理はO(1)ですが、final selectionの再検証、grab時のZ順保持、release時のsnapとstate commitには明示的な大量処理が残ります。極端な重なりではrasterとpickingの負荷が増えます。異OS/GPU、通常windowの全手動操作は今後の確認対象です。
