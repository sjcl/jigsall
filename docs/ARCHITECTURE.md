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
| `core/src/connectivity.rs` / `snapping.rs` | DSUと循環member list、正しいgrid隣接、同rotationのtranslation candidateの決定 |
| `puzzle/src/procedural.rs` | u32 hash、packed EdgeProfile、解析形状・UVのCPU参照 |
| `puzzle/src/fingerprint.rs` | feature / test限定のmacro fingerprint、輪郭descriptor、凍結v4測定参照 |
| `puzzle/src/placement.rs` / `grid.rs` | O(N)格子リング配置、seed付きshuffle、grid |
| `puzzle/src/shapes.rs` / `generation.rs` | feature / test限定のv2 Bezier・lyon・Rayon・U16 geometry |
| `game/src/resources/pieces.rs` | 16-byte dense正本、dense owner IDs、selection / dirty mask、bulk authority、drag bitset / delta、dirty upload |
| `game/src/resources/pieces/snapping.rs` | Release単位の単一snap判定、固定offsetのunion closure・一括配置 |
| `game/src/interaction.rs` / `systems/piece_interaction.rs` | 非同期選択のgesture、命令発行、矩形overlay |
| `game/src/systems/game_logic.rs` | 命令適用、Release後のsnap、イベント駆動の進捗 |
| `game/src/network/runtime.rs` / `runtime/` | Direct-IP session lifecycle、local command bridge、World同期、切断とMenu cleanup（[仕様](DIRECT_IP_RUNTIME.md)） |
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

player一覧の正本は `game/src/players.rs::PlayerRoster` です。`GameData` は進捗専用です。
display name は core の validated `PlayerDisplayName` で、protocol identity の
`PlayerId` や将来の platform account ID と独立しています。Direct-IP は ReadyCommit の
完全 snapshot と Reliable presence を使い、offline は local preference から初期化します。
設定は General から draft を検証して `settings.json` の `player` section へ保存します。
roster は session metadata で、snapshot / save / piece state へ含めません。
詳細は [Direct-IP runtime](DIRECT_IP_RUNTIME.md#player-profiles-and-presence) を参照してください。

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

永続連結は`PieceConnectivity`の`Vec<i32> parent_or_size`と`Vec<u32> next_member`で表現します。union-by-sizeとpath compressionを使い、循環listのsuccessor交換でmember listをO(1)結合します。余剰bitsに最小member IDを保存し、offsetの代表とRelease処理順をsnapshot復元前後で揃えます。100万ピースで追加8,000,000 bytes、componentごとのEntity / 恒久member Vecはありません。隣接はrow-major IDから上下左右だけを導出します。rotation == 0でRelease直後のoffsetがstrict threshold内ならboardを優先しZEROへ配置します。範囲外の場合だけ、同rotationの正しい隣接componentから最小offset距離、tieなら最小member PieceIdの順にtargetを1つ選びます。moving componentを一度だけ正規化し、以後のoffsetは固定します。固定final offsetへの再構成をmatches_transformで検証し、f32の算術丸めだけを許容した同rotation・同一translationのvalid・unheldな隣接componentをclosureへ加えます。targetの座標は動かしません。Release共通scratchは少数IDをstackへ保存し、容量を超えたsetだけdenseへ昇格します。target検証と解決済みrootのlogical offsetをcacheし、成長するcomponentの再走査を抑えます。scalar Releaseはrootを直接処理し、Group Releaseも全componentのauthority検証後にaccepted maskを再構築せずmemberを処理します。詳細・計算量・計測・制限は[CONNECTED_SNAPPING.md](CONNECTED_SNAPPING.md)を参照してください。

入力はPostUpdateのegui処理、camera pan / zoom / edge scrollingの後です。現Transformで座標変換し、UI上の押下を抑制します。開始済みdragはUIを横切っても継続・解放できます。pauseとfocus lossで保持を解放し、未確定の矩形選択を元に戻します。

ドラッグ中のQ/EはRotateDragで表示中のdeltaと回転をcanonical stateへ一度に確定し、成功後だけpointer anchorを現在pointerへ更新します。対象全体をpreflightし、拒否時はstate / delta / anchorを維持します。Releaseと同frameならReleaseを優先し、回転中にはsnapしません。pointer dragはmembers + deltaのCPU O(1)、state / membership upload 0を維持し、明示的なdrag rotation時だけO(k)の計算と変更memberだけのuploadを行います。GPU stateは16 bytes、component root bufferとmembershipは回転で変更しません。詳細は[ROTATION.md](ROTATION.md)を参照してください。

`PieceInteraction`はIdle / Dragging / BoxSelectingを持ちます。point結果の受信前にreleaseした場合も最終座標を保持します。矩形previewとrelease時の確定要求を分け、古いGPU応答が確定選択を上書きしないようにします。

Direct-IP clientのRelease待ちでは、gesture終了後も`CommandBridge`がaccepted membershipと送信したfinal_delta / tokenを保持し、既存`PieceDataStore.drag`のCOW maskとscalar deltaでlocal presentationを継続します。pointer/cameraから再計算せず、新しいpiece gestureだけを解決まで抑制します。RotateDrag待ちのqueued ReleaseはACKでbasis補正したdeltaを表示し、実送信値と一致させます。`PeerReplicationState`のReliable canonical commit後にpendingを解除し、Last / extraction前に整合させます。active local / pending local / remote Transient / canonicalの違いとfailure・scope・session cleanupは[DIRECT_IP_RUNTIME.md](DIRECT_IP_RUNTIME.md#local-release-presentation-while-awaiting-authority)を参照してください。pendingは保存・snap・進捗に使用しません。

## Dirty同期とZ順序

### Local uncommitted rotation presentation

描画poseには4層があります。CPU `PieceDataStore.states` はauthorityがcommitした
canonical poseです。`store.drag` はlocal active / pending Releaseのmembershipとscalar
translationです。`store.local_rotation` はlocal未ACK Rotate / RotateDragだけの疎な
pose overrideです。`RemoteDragPresentation` はremote Transientの平滑化されたtranslation
です。local回転をremote PlayerId slotへ登録しません。

network clientのQ/Eでは`CommandBridge`の送信待ちcontrolとin-flight controlを順に再生します。
in-flightはReliable envelopeのControl sequence、送信待ちはqueueの順序、各controlは
gesture token / target / turns / pointer / deltaで対応します。authority ACKでcanonicalを
applyした後、matching controlだけをcommitted prefixとしてretireし、残るuncommitted
predicted suffixを新canonical baseへreplayします。部分ACKでaccepted操作の表示を
過去のlocal intentへ巻き戻しません。PreUpdateのexclusive network poll内でcanonical
apply → bridge rebase/retire → presentation再構築を完了し、PostUpdate入力後のlocal
controlもLast/upload前に反映します。renderer extractionに途中のcanonical-only poseを
公開しません。

authorityとpredictionは同じ`rotation_plan_with` / `RotationPlan::pose`を使い、component
pivot、f64 translation、quarter turn、play-areaの丸め補正、canonical座標からの再構成を
共有します。predictionは`DensePieceStates`をclone/mutateせず、affected PieceIdだけの
HashMapにposition / rotationを保持します。ownership、Z、connected edges、ENABLED / HELD /
PLACED、snap、progress、snapshot/save、authority cursorとreplicaはcanonicalのままです。

RotateDragのreplayはqueued controlのACK補正済みdeltaを用います。表示baseから未ACK
rotationが消費したtranslationを差し引くため、現在の`store.drag.delta`をshaderで一度
加えるだけでpointerが即時追従します。protocol basis / through_tick / pointer anchorは
ACKだけで更新します。Release後も同tokenの未ACK回転を保持し、各rotation ACKで補正された
pending Release deltaと合成します。ReleaseCommittedのcanonical apply後にtranslationと
回転suffixを解除します。authority snapによる最終補正は表示へ反映します。

Grab ACK前はrequested membersをoptimisticに使えますが、partial GrabAccepted後はexact
accepted membershipだけを使います。通常Rotateのpartial acceptanceではaccepted componentを
canonicalへhandoffし、競合でinvalidになったcomponentだけを戻します。tokenが異なる古い
ACKは新gestureのpointer/presentationをrebaseしません。cancel、rejection、send/protocol
failure、disconnect、scope/epoch change、baseline/Ready、stop/Menuでoverrideをrestoreします。
hostは同期canonical applyだけを使い、offlineはprediction queueを使いません。

Lastの`prepare_piece_upload`はdirty rangeのcanonical stateへpositionとrotation bitsだけを
合成します。退役/拒否時は旧override membershipもdirtyにしてcanonicalへ確実にrestoreします。
新GPU bufferだけは共有canonical initial Arcを先にuploadし、疎なpresentation rangesを後に
uploadします。既存state bufferをmain / far / visibility / point / rectangleで共有し、別の
完全GPU buffer、shader分岐、piece Entity / Meshは追加しません。

回転inputとReliable reconciliationだけがaffected membershipを走査します。replayは最大
64 queued controls + 1 in-flight controlに制限され、各controlはO(k)のplanner処理です。
1M対象なら一時pose map / plan / uploadも対象数に比例しますが、1piece操作でcanonicalの
16MBをCOW copyしません。ordinary pointer/camera、pending ACK idle frameはpose mapを
走査/再構築せず、state/membership uploadは0 bytesです。wire 9、snapshot schema 4、
JoinBaseline schema 1、GpuPieceState 16 bytesを維持します。

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

画像workerは元画像からdevice非依存のlogical size（最大辺16384 px、整数比率・最近傍の寸法丸め）を計算し、次にその端末のGPU辺上限と画像メモリ予算へ収まるtexture sizeを計算します。workerで`into_rgba8`により最終GPU形式へ変換した後、`fast_image_resize`のU8x4 / Lanczos3によりtexture sizeへ一度だけresizeしてmain threadへ渡します。16bit入力も縮小前に8bitへ量子化します。従来と同じ独立したRGBA channel補間を使い、alpha乗除算による元画像サイズの追加コピーを作りません。縮小不要で既にRGBA8ならpixel領域を再利用します。`PuzzleImage.logical_size`がゲーム定義・生成・カメラ・背景Sprite・UIの座標系で、`texture_size`は描画用の端末別解像度です。寸法を毎frame textureから上書きする経路はありません。main / point / rectangleは同じ縮小textureと正規化UVを共有し、shaderにdevice寸法を持ち込みません。

Startupで使用中のRenderDevice / RenderAdapterから`PuzzleImageLimits`を取得し、画像選択と保存loadのrequestには予算から算出した上限値だけをコピーします。workerはGPU resourcesへアクセスしません。予算は既定でGPU容量の20%を使う自動モードで、割合・手動予算をSettingsから変更できます。取得経路、fallback、設定の適用時期は[SETTINGS.md](SETTINGS.md)を参照してください。元encoded bytes / SHA-256は縮小と独立して保持します。

共通decoderはencoded入力512 MiB、各辺32768 px、総画素数67,108,864（8192²）以下を要求します。decoderのヘッダーから寸法を確認し、画素bufferの確保前に超過を拒否します。この制限は画像選択・保存load・サムネイル・ネットワークの転送済み／cache画像すべてに適用します。24000×16000などの超大画像と、それを元画像に持つ既存saveはload時にエラーになります。24000×16のような総画素数の少ない画像は引き続き縮小できます。decoderのallocation limitは512 MiBで、出力分を明示的に予約してからdecodeします。ただしcodec内部のallocation limitはbest-effortです。

RGBA8の元画像・出力・縮小の中間画素bufferはそれぞれ最大256 MiBです。中間bufferは`元画像の幅 × 縮小後の高さ × 4 bytes`（alignmentを除く）以内で、従来のRgba32F / 16 bytesではありません。16bit RGBAのdecode結果は最大512 MiBで、RGBA8への変換中は両bufferが存在します。縮小の画素buffer合計は最大768 MiBですが、encoded bytes、codec内部、補間係数、allocator、別workerの同時処理、GPUの使用量は含みません。process全体のメモリ上限や実測RSSを示す値ではありません。

画像は`RenderAssetUsages::RENDER_WORLD`を使い、Bevy 0.19.1のextractがpixel Vecをrender worldへ移します。GPU upload後にCPU pixelデータは保持しません。main worldには寸法metadataとhandle、opaque判定を残し、背景Spriteとpieceが同じGPU textureを使います。4096² RGBA8画像のCPU常駐64 MiBとextract時の同サイズのcloneを削減します。

永続連結の追加後もdense stateの受け取り・初回uploadはcopy不要ですが、`initialize_dense`は新しい8-byte / pieceのDSU領域をO(N)で初期化します。上記0.0024 msはDSU導入前の受け取り測定で、現在の初期化コストは[CONNECTED_SNAPPING.md](CONNECTED_SNAPPING.md)のmetadata計測を参照してください。通常idle / pointerにこの処理はありません。

完成画面ではパズルを残します。GameCompleteのsubstateで結果カード・完成盤面の閲覧・ESCメニューを切り替え、InGameへ再入場せず定義・画像・配置を保持します。完成時に一度だけカメラを中央へ合わせ、閲覧中はpan / zoomを有効にします。Menuへ戻る際、定義・state・画像・背景・選択・gesture・overlay・worker受信器・命令を清掃します。epochでGPU stateを作り直し、request IDをセッション間で再使用せず、前セッションの遅延readbackを無効にします。

## GPU presentation

### Remote drag presentation

Direct-IP hostとclientは、game-layerの`network/runtime/presentation.rs`で検証済みcontextを`RemoteDragPresentation`へ変換します。hostは`ProtocolDragContexts` / `HostCommandOutcome`、clientは`PeerReplicationState`と成功したauthority / Transient routeを使用します。rendererはnetwork runtime型に依存せず、offlineでも同じ空のresourceを使います。

`PieceDataStore`がcanonical position・ownership・rotation・Z・connectivityの正本です。canonical positionはReliable authority commitでだけ変わり、networkがacceptedした`RemoteDragUpdate.delta`はpresentationの最新targetです。GPUが使うdeltaはCPUで平滑化したdisplayed deltaで、表示位置はcanonical position + displayed deltaです。Transient / smoothing frameは`GpuPieceState`、snapshot、save、progress、snap、authority cursorを変更せず、smoothed valueを`PeerReplicationState` / `ProtocolDragContexts`へ戻しません。local active pointerとpending Releaseは従来の`store.drag.members` / GPU bitset / `PuzzleUniform.drag_delta`を使用し、local playerはremote slotへ登録せず即時feedbackを維持します。HELDによる選択除外とcanonical Zを維持します。

remoteは`u32 piece_slots[N]`（0=無し、1..64=slot）と512-byte固定delta uniformです。uniformは2つのVec2を1つのvec4にpackします。GPU追加常駐は4N + 512 bytes、100万pieceで4,000,512 bytes（約3.815 MiB）。CPU cacheは4N-byte mapping、dirty bitset（100万で125,000 bytes）、最大64のmembership bitsetとdeltaです。Dense accepted targetのbitsetはArc共有し、SparseはGrab境界でだけbitsetへ展開します。Sparseの新規bitsetは1 slotあたり最大125,000 bytes、全64 slotで最大8,000,000 bytesです。upload snapshot / rangesのpayloadは別途保持し、dense時は最大4N bytesです。PlayerIdはgame-layerの最大64件のslot lookupだけにあり、GPUはPlayerIdを検索しません。

Reliable `GrabAccepted`でauthorityのexact accepted membershipを割り当て、displayed / targetをcontextの現在deltaへ即時一致させます。partial acceptanceも要求targetではなく受理済みcontextを参照します。最初の空slotを再利用し、前playerのtarget・displayed・smoothing ageを引き継ぎません。Release / Cancelでは保存した正確なbitsetを使ってmappingを0にし、両deltaとsmoothing stateを直ちに破棄します。ReleaseのsnapでDSUが結合しても、旧membershipは変わりません。lagが残っていてもReliable canonical final positionへ即時handoffし、post-release animationは行いません。`DragRotationCommitted`ではmembershipを維持し、Reliable rebase後のnew-basis context deltaへdisplayed / targetを即時一致させます。旧basisから補間せず、旧basis / release / cancel後、duplicate / staleのTransientは既存benign-drop contractで落とし、targetを変更しません。

通常Transientはplayer→slot lookupと最新target更新だけでO(1)です。同frameのburstは最後のaccepted targetを上書きし、sample queueは作りません。Lastの`prepare_remote_drag_upload`は全network poll / command処理の後、epoch同期 → smoothing advance → upload準備の順で実行され、ExtractScheduleより前に同frameのtargetを表示へ反映します。`Time<Real>`のframe deltaを使うためvirtual pause / slow-motionに依存せず、wire tickを時間として解釈しません。

`resources/remote_drag.rs::REMOTE_DRAG_SMOOTHING`へ調整値を集約しています。`display += (target - display) * (1 - exp(-dt / tau))`、tau = 25 msで、約75 msで95%、100 msで98%追従します（固定targetに対する式の値）。各軸は現在displayとtargetの間へ収まり、direction reversalでもvelocityの慣性やovershootはありません。prediction / extrapolation / RTT補償 / clock同期は行わず、packet loss中も最後のtargetへ向かうだけです。各軸の残差が0.001 world units以下、またはtarget変更から250 ms経過したframeでexact settleします。250 ms以上のframe hitchは即時snapです。

target / displayed / smoothing ageと64-bit active maskはmappingから独立した再構成可能なCPU cacheです。追加CPU領域は固定1,032 bytes（targets 512 + age 512 + mask 8、paddingを除く）。idleはactive maskで即returnし、frame advanceは最大64 slotだけを見ます。piece / membership / `PieceDataStore.states`走査、mapping再構築はありません。Lastでdirty mappingだけを連続rangeにまとめ、最大128 spansを超えたらenclosing rangeを1回uploadします。smoothingが変えるのはdisplayed delta revisionだけで、active frameのcanonical / mapping / membership uploadは0 bytes、remote delta uploadは最大512 bytesです。settle後の次frameではdelta revisionも止まり、両remote uploadは0 bytesです。membershipのO(N)処理はReliable control / initialization境界に限ります。piece Entity / Meshは追加しません。

client ReadyではJoinBaseline / catch-up / FinalDragSet reconciliationが完了した**current** replica contextからmembershipを構築し、displayed == target == reconciled deltaへ即時初期化します。初回Transientを待たず、過去のdragをzeroからanimationさせず、final scalar rollbackもそのまま表示します。store epoch / authority scopeの変更、join baseline / new session、snapshot / new puzzle、Menu / session stop / host lossでmapping・membership・dirty ranges・両delta・smoothing stateをresetし、GPU revisionを進めます。renderer bufferはpiece epochとともに作り直し、remote mapping / deltaのrevisionが一致した後に描画・RenderReadyを進めます。

`presentation.wgsl::presentation_position`はmain visibility、pick ROI visibility、normal / far-splat vertexに共通です。point / rectangleは同じvertexを使います。canonical HELDを前提にlocal membershipを優先し、remote translationを重ねて二重移動させません。wire v9、`GpuPieceState` 16 bytes、snapshot schema 4、join baseline schema 1は変更しません。

接続componentのselection / preview outlineは、dense stateのflags bit 5–8にあるtop / right / bottom / leftの接続cacheを使って内部辺を除外します。cacheはDSUの派生情報で、既存snap closureのneighbor探索内で両側をincrementalに更新し、変化したpieceだけdirtyにします。16-byte stateを維持し、snapshot schema 4のinstallでは復元DSUからcacheを再構成します。fragmentは4辺SDFを一度だけ計算し、coverage / pickingは全辺、黄 / 青outlineは共通の未接続境界を使います。全4辺が接続した内部pieceにoutlineはありません。

rectangleはselectableなdirect hitだけをmaskへrasterし、preview中だけ1回のGPU computeでcomponent rootのmaskへcollapseします。component atomicなauthority更新とvalidated restoreにより、正規状態のselectabilityはcomponent内で揃います。main vertexがpreview中だけrootとpreview maskを読み、結果のPREVIEW bitを既存のflat flagsでfragmentへ渡します。root用varyingは追加せず、pick用uniformはpreview_activeを0にしてselection rasterのroot参照も避けます。final readbackは従来のdirect hit bitsetで、CPUのcommit_selectionがcomponent全体を再検証・確定します。GPU root bufferは4 bytes / piece、CPUにはroot dirty bitsetだけを持ち、unionでabsorbed memberをdirtyにして最終rootをrange uploadします。initial / restore時だけDSUから全rootを生成します。idle / camera / pointer dragでroot scan・root upload・preview computeはなく、preview_active == 0ならvertexもrootを参照しません。pipelineとメモリ・計算量は[GPU_PICKING.md](GPU_PICKING.md)に記載しています。

Core2d main transparent pass後のカスタムpassです。背景画像Spriteは通常Bevy描画。GPUは拡張quad AABBでvisible IDとindirect argsを生成し、mainはdraw_indirect1回です。4頂点はvertex_indexから作り、vertexで4辺を2 u32ずつ生成してflat varyingへ渡します。fragmentはSDF・画像alphaでdiscardし、UV・outlineを評価します。

opaqueは任意のinstance順でdepth test/write、半透明は可視IDだけをGPU radix sort（8bit × 3 pass）で後方→前方に並べblendし、depthを書きません。透明経路ではID順に可視IDを圧縮してから安定sortし、同じZのID順も維持します。workgroup数はGPUのinstance_countからindirect dispatchで決め、CPU readbackは不要です。matrix・state・visibleをpickingにも共有します。矩形overlayは追加draw1回です。sortは[TRANSPARENT_RADIX_SORT.md](TRANSPARENT_RADIX_SORT.md)、選択は[GPU_PICKING.md](GPU_PICKING.md)に記載しています。

## Persistent save の境界

ローカル進捗保存は `PieceDataStore / PuzzleDefinition → PuzzleCheckpoint → PuzzleSave → SaveCodec → SaveRepository → SaveStorage → FilesystemStorage` の流れです。画像は選択時の original encoded bytes を SHA-256 で識別し、再エンコードしない `.puzimg` を save 間で共有します。進捗 `.puzsave` v3 は 16 bytes/piece の明示的 little-endian codec と全体 / header checksum を使い、ランダム SaveId で保存します。新規ゲームにはUUID v4のGameIdを発行し、すべての手動保存・オートセーブとそのロードで引き継ぎます。オートセーブは同じGameIdの最新指定件数（既定1件）を保持し、新しい保存の成功後に古い履歴を削除します。最大509 bytesの header とファイル長だけで一覧とローテーション対象を判別し、非 authority の placed_count cache は完全 load 時に state と照合します。旧 format の互換コードは持たず、対応 version は3だけです。更新 request は読み込んだ revision を保持し、現在の header と不一致なら Conflict として publish 前に拒否します。ユーザータイトルは validation を持つ metadata で、filename / identity には使いません。

`GameSnapshot` schema 4は16-byte piece layoutを保持し、borrowed checkpoint view を通じて同じ capture / validation / install を使います。restore は DSU / 接続 GPU cache / placed_count を再構築し、GameData の progress / completion を同期します。load worker が準備した store を直接採用するため、random 初期配置は生成しません。既存 epoch / RenderReady による GPU 準備待ちの後だけ Playing / GameComplete へ遷移します。disk I/O、decode、codec は worker/channel に分離し、O(N) capture は明示 Save 時だけです。通常 play に新しい piece 数比例の処理や per-piece Entity / persistent Vec は追加しません。

保存先は OS user application data 以下で、logical key を storage に渡します。画像を先に保存し、save は temporary file の sync と atomic replace で publish します。通常 Save は共有画像の存在だけを確認し、import / load で画像全体の hash を検証します。SaveStorage は Send / Sync を要求せず、filesystem は worker で動かします。メニューのサムネイルは専用queue / workerと独立したfilesystem handleで読み込み・検証・decodeし、保存・ロードのworkerを占有しません。ImageHashで共有する最大64件の成功texture cacheをメニュー退出・session変更後も保持します。将来の Steam Cloud は handle を所有 thread に保持し、両workerからのStorageRequestsのoperationを非同期 API に dispatch、callback から返信する executor を追加します。StorageProxy を使う repository / codec / restore 準備は worker 上で継続します。write は encoded Vec の所有権を移譲し、proxy による全 blob コピーを避けます。Steam Cloud 自体は未実装です。形式・layout・failure / worker lifecycle の詳細は [PERSISTENCE.md](PERSISTENCE.md) を参照してください。

FilesystemStorage は Rust 標準のファイルロックで複数プロセスを協調させます。画像ごとの共有ロックを `ImageLease` として取り込み・ロードの応答から `OriginalPuzzleImage` へ移し、元 encoded bytes の解放後も使用中の画像を保持します。保存待ちの request も lease を共有し、画像の置き換え・session cleanup・古い応答の破棄で解放します。save に未参照の画像を掃除する場合は画像の排他ロックを待たずに試し、使用中なら見送ります。共通の repository 排他ロックは画像 import、save の revision 検証から公開、autosave ローテーション、delete の参照確認から掃除までを直列化します。専用 lock file は削除・置換せず、取得の再試行待ちは worker だけで行い、StorageProxy / executor には非 blocking の試行を渡します。プロセス間の直接通信はありません。

## Multiplayerの境界と課題

PieceIdはEntity IDから独立したu32、PlayerIdはu64です。version、seed、grid、画像寸法で形状を再構成します。core/sessionはsession identity・画像hash・命令sequence・authority epoch・migrationを、game/multiplayerはsnapshotの検証・復元とplayer単位の保持解放を提供します。GrabGroup / ReleaseGroupも同じ認証済みplayerとreliable control streamを使います。selectionはlocal presentationでありsnapshotには入りません。Direct-IP transport、途中参加、レート制限、Host / Join UIは[DIRECT_IP_RUNTIME.md](DIRECT_IP_RUNTIME.md)を参照してください。Host UIはCPU storeとGPUの準備後にlistenし、Join UIは専用接続画面でNetworkStatusを表示します。

transport向けにはcore/protocolのComponentRef / PieceTarget / ProtocolPieceCommandを使用します。minimum memberとexpected sizeでcomponentを参照し、32 componentまでcompact、より多いselectionは対象componentのcount / topology digest付きDenseへ切り替えます。game/multiplayer/protocolのopt-in authority adapterがcurrent connectivity・所有権・placed・enabledを再検証し、Grabで受理した結果だけをSparse / Denseのplayer別contextへ保持します。Denseはcomponent listへ展開せずcanonical bitsetを保持し、受理したmembershipをGrabAccepted ACK / authority event型で返します。既存Move sequenceはmembership不要のbest-effort DragUpdateにも共用し、ReleaseはGrab sequenceとfinal deltaだけで確定します。local PieceCommand / PieceBitSetとGPU経路は維持します。詳細は[MULTIPLAYER_PROTOCOL.md](MULTIPLAYER_PROTOCOL.md)を参照してください。

snapshot schema 4は16-byte stateのflagsへPLACED / CONNECTED_RIGHT / CONNECTED_DOWN / rotation（bit 9–10）を保存します。root IDはprotocolへ保存せず、install時に隣接edgeからDSUを再構成します。schema 1 / 2 / 3、境界外edge、placedの誤座標、component内のrotation / rigid transform / placed不一致は変更前に拒否します。restoreはpositions / Z / connectivity / placedを保ち、holds / selection / dragをresetします。disconnectはcomponent全体のholdだけを解放し、位置とsnapを変更しません。

GPUは描画と選択の補助で、placed・所有権・snapを決めません。cullingはO(N)、半透明sortは可視数に比例します。選択保持・drag pointer・rectangle previewのCPU処理はO(1)ですが、final selectionの再検証、grab時のZ順保持、release時のsnapとstate commitには明示的な大量処理が残ります。極端な重なりではrasterとpickingの負荷が増えます。異OS/GPU、通常windowの全手動操作は今後の確認対象です。
