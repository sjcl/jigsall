# Jigsall のアーキテクチャ

2026-10-06。基準`22e0aa135c5bdc6a881a3fe2ab6d976087d728ba`のnative lyon generator v2からprocedural GPU rendererへ移行済みです。移行検証用の旧 CPU メッシュ生成と CPU picking は削除しました。現在は開発時v4の楕円弧の付け根を保ちながら辺の識別性を高めたgenerator v1（開発時v5）です。初回リリース向けにgeneratorとsnapshot schemaをそれぞれ5→1に整理し、生成結果とsnapshotのlayoutは維持しています。開発中の形式との互換性や移行は提供しません。v3移行時の数値は[PROCEDURAL_RENDERER.md](PROCEDURAL_RENDERER.md)、付け根修正は[ROOT_TRANSITION.md](ROOT_TRANSITION.md)、現在のclass decodeと検証結果は[EDGE_FINGERPRINT.md](EDGE_FINGERPRINT.md)を参照してください。

## Workspaceと責務

```text
jigsall
  ├── jigsall-ui → jigsall-game / jigsall-puzzle / jigsall-core
  └── jigsall-game → jigsall-core / jigsall-puzzle
                                         └── jigsall-core
```

共通の依存バージョン・Cargo.lock・targetをworkspaceで管理し、全packageをdefault-membersに含めています。

| ファイル | 責務 |
| --- | --- |
| `core/src/gameplay.rs` / `commands.rs` | row-major PieceId、PuzzleDefinition、命令型・座標定義 |
| `core/src/connectivity.rs` / `snapping.rs` | DSUと循環member list、正しいgrid隣接、同rotationのtranslation candidateの決定 |
| `puzzle/src/procedural.rs` | u32 hash、packed EdgeProfile、解析形状のCPU参照 |
| `puzzle/src/fingerprint.rs` | shape-analysis / test限定のmacro fingerprint、輪郭descriptor、凍結v4測定参照 |
| `puzzle/src/placement.rs` / `grid.rs` | O(N)格子リング配置、seed付きshuffle、grid |
| `game/src/resources/pieces.rs` | 16-byte dense正本、dense owner IDs、selection / dirty mask、bulk authority、drag bitset / delta、dirty upload |
| `game/src/resources/pieces/snapping.rs` | Release単位の単一snap判定、固定offsetのunion closure・一括配置 |
| `game/src/interaction.rs` / `systems/piece_interaction.rs` | 非同期選択のgesture、命令発行、矩形overlay |
| `game/src/systems/game_logic.rs` | 命令適用、Release時のsnap、正本に基づく進捗 |
| `game/src/network/runtime.rs` / `runtime/` | Direct-IP session lifecycle、local command bridge、World同期、切断とMenu cleanup（[仕様](DIRECT_IP_RUNTIME.md)） |
| `game/src/network/address.rs` | 参加先の構文検証とworkerによるDNS解決、timeout、IPv4 / IPv6 endpoint選択 |
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

旧 v2 の Bezier・lyon tessellation、CPU triangle / R-tree picking、専用 example と feature を削除し、lyon・lyon_tessellation・Rayon・rstar の依存を除去しました。現行 shader との比較に使う解析 SDF と形状評価 example は維持します。`jigsall-puzzle/shape-analysis` は fingerprint 評価用です。入力テストは選択結果を明示的に注入し、実 GPU の coverage は render tests で検証します。

## CPU正本と入力

新規ゲームの回転トグルは開始時に`PuzzleDefinition.rotation_enabled`へ固定します。
オンでは`puzzle::placement::initial_rotation`がseedとPieceIdから四方向を決め、
既存workerが16-byte stateのrotation bitsへ格納します。長方形の回転を含むslot寸法を
配置・カメラ・LogicalPlayAreaで共有します。オフでは入力・authority・prediction・replicaが
回転を禁止し、保存／snapshotも非ゼロrotationを拒否します。詳細は[ROTATION.md](ROTATION.md)。

### Remote cursor presentation

Direct-IPのcursorはgame/network限定のsession-only presenceです。
`network/cursor.rs` と `runtime/cursors.rs` がworld-space CursorUpdate（20 Hzの
Transient heartbeat）を認証済みReady接続のPlayerIdに対応付け、host自身のsampleと
合わせて最大65人のvisible setを1つのCursorSnapshotへbatchします。host→各Ready
clientも20 Hzです。tick / sequenceは単調でwrapせず、旧session / epoch / stale /
duplicateを無視します。full snapshotから消えたentryはhiddenになり、packet lossは
次のheartbeat / full snapshotで自己修復します。Syncing中のReadyCommit追い越しと
roster未登録IDはbenign dropです。名前の正本はPlayerRosterで、名前・色・cameraをwireへ
送りません。None・pause・focus loss・window外はhidden、hideは即sample、失効は400 msです。
empty snapshotは初回と全員hiddenへの遷移時に1回だけ送り、visible cursorが戻るまで停止します。
最後のemptyが失われた場合もclientの400 ms expiryで表示を消します。

`resources/remote_cursor.rs::RemoteCursorPresentation` はpieceに触れず、最大player数だけの
target / displayed world positionを持ちます。`Time<Real>` の指数平滑化（25 ms）でtargetへ
収束し、epsilonまたは250 msでexact settleし、settled frameの変更検知を進めません。
prediction / extrapolationはありません。`ui/src/remote_cursor.rs` はroster名とlocalized fallbackを
低頻度にCPU rasterizeして、bounded R8 texture atlasを作ります。既存の日本語font bytesを共有します。
`render/remote_cursor.rs` はExtractScheduleでdisplayed world positionとatlas revisionを受け取り、
puzzleの同frameの`clip_from_world` / viewportを共有する専用GPU marker・label passで描画します。
pan / zoomで新packet・instance upload・atlas rebuildは不要です。logical sizeは一定で、DPI変更時に
atlasを再生成します。puzzle / selection boxの後、egui HUD / menuの前で、Playing時だけ表示します。
input capture / focusを登録しません。詳細とboundsは[GPU cursor presentation](REMOTE_CURSOR_GPU.md)を参照してください。

cursorの通常処理は最大64 remote playersのsmoothing / instance描画で、100万pieceのstate・membership・
component・piece GPU upload・dirty revisionへ接点を持ちません。authority cursor、gameplay protocol、
snapshot / checkpoint / JoinBaseline / catch-up / save / autosaveから完全に分離します。
disconnect / PlayerLeftは即削除、session / epoch / baseline / Ready replacementとteardownは
全resetです。wire v1の最大snapshotは1,210 bytes、Transient上限は1,280 bytesです。
詳細は[runtime](DIRECT_IP_RUNTIME.md#remote-cursor-presence)と
[transport](NETWORK_TRANSPORT.md#world-space-remote-cursors)を参照してください。

player一覧の正本は `game/src/players.rs::PlayerRoster` です。`GameData` は進捗専用です。
display name は core の validated `PlayerDisplayName` で、protocol identity の
`PlayerId` や将来の platform account ID と独立しています。Direct-IP は ReadyCommit の
完全 snapshot と Reliable presence を使い、offline は local preference から初期化します。
設定は General から draft を検証して `settings.json` の `player` section へ保存します。
roster は session metadata で、snapshot / save / piece state へ含めません。
詳細は [Direct-IP runtime](DIRECT_IP_RUNTIME.md#player-profiles-and-presence) を参照してください。

`PieceDataStore.states: DensePieceStates`が正本です。内部は固定長の`Arc<[GpuPieceState]>`で、`PieceId(n)`は`states[n]`を直接参照します。position、u32 z_order、flagsの16 bytesです。grid位置、正解位置、size、UV、辺パラメータ、boundsは定義とIDから導出します。全ピース分の component や Transform は保存しません。旧 `PuzzlePiece`・単体 state 用命令検証 / snap API と、発行元のない配置通知経路も削除しました。命令適用は `PieceDataStore::apply_command`、進捗更新は正本の `placed_count` を参照します。確定選択とdirty IDは`PieceBitSet`、holderはdense PlayerIdとoccupancy maskです。矩形previewはGPU bitsetを直接outlineへ利用し、release時だけCPU maskへreadbackします。drag中の一時移動は固定membership bitsetとdeltaで表現し、最終座標だけをrelease時にCPU正本へ反映します。[MILLION_SELECTION.md](MILLION_SELECTION.md)に移行・計測・メモリを記載しています。

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

新規ゲームではcoreの`PuzzleDefinition::new`が`min(image_width / columns, image_height / rows) / 5`でsnap距離を決定します。論理画像寸法と確定gridだけを使い、seed・GPU textureの縮小・camera zoomに依存しません。固定の最小距離・最大距離を設けず、細かいgridでも短辺の20%を維持します。作成画面には調整項目を設けず、計算結果を`PuzzleDefinition.snap_distance`へ凍結してmultiplayer / saveで共有します。既存saveの復元とnetwork joinでは記録・受信した距離を維持します。

永続連結は`PieceConnectivity`の`Vec<i32> parent_or_size`と`Vec<u32> next_member`で表現します。union-by-sizeとpath compressionを使い、循環listのsuccessor交換でmember listをO(1)結合します。余剰bitsに最小member IDを保存し、offsetの代表とRelease処理順をsnapshot復元前後で揃えます。100万ピースで追加8,000,000 bytes、componentごとのEntity / 恒久member Vecはありません。隣接はrow-major IDから上下左右だけを導出します。rotation == 0でRelease直後のoffsetがstrict threshold内ならboardを優先しZEROへ配置します。範囲外の場合だけ、同rotationの正しい隣接componentから最小offset距離、tieなら最小member PieceIdの順にtargetを1つ選びます。moving componentを一度だけ正規化し、以後のoffsetは固定します。固定final offsetへの再構成をmatches_transformで検証し、f32の算術丸めだけを許容した同rotation・同一translationのvalid・unheldな隣接componentをclosureへ加えます。targetの座標は動かしません。Release共通scratchは少数IDをstackへ保存し、容量を超えたsetだけdenseへ昇格します。target検証と解決済みrootのlogical offsetをcacheし、成長するcomponentの再走査を抑えます。scalar Releaseはrootを直接処理し、Group Releaseも全componentのauthority検証後にaccepted maskを再構築せずmemberを処理します。詳細・計算量・計測・制限は[CONNECTED_SNAPPING.md](CONNECTED_SNAPPING.md)を参照してください。

入力はPostUpdateのegui処理、camera pan / zoom / edge scrollingの後です。現Transformで座標変換し、UI上の押下を抑制します。開始済みdragはUIを横切っても継続・解放できます。pauseとfocus lossで保持を解放し、未確定の矩形選択を元に戻します。

ドラッグ中のQ/EはRotateDragで表示中のdeltaと回転をcanonical stateへ一度に確定し、成功後だけpointer anchorを現在pointerへ更新します。対象全体をpreflightし、拒否時はstate / delta / anchorを維持します。Releaseと同frameならReleaseを優先し、回転中にはsnapしません。pointer dragはmembers + deltaのCPU O(1)、state / membership upload 0を維持し、明示的なdrag rotation時だけO(k)の計算と変更memberだけのuploadを行います。GPU stateは16 bytes、共有metadataのcomponent root領域とmembershipは回転で変更しません。詳細は[ROTATION.md](ROTATION.md)を参照してください。

`PieceInteraction`はIdle / Dragging / BoxSelectingを持ちます。point結果の受信前にreleaseした場合も最終座標を保持します。矩形previewとrelease時の確定要求を分け、古いGPU応答が確定選択を上書きしないようにします。

Direct-IP clientのRelease待ちでは、gesture終了後も`CommandBridge`がaccepted membershipと送信したfinal_delta / tokenを保持し、既存`PieceDataStore.drag`のCOW maskとscalar deltaでlocal presentationを継続します。pointer/cameraから再計算せず、新しいpiece gestureだけを解決まで抑制します。RotateDrag待ちのqueued ReleaseはACKでbasis補正したdeltaを表示し、実送信値と一致させます。`PeerReplicationState`のReliable canonical commit後にpendingを解除し、Last / extraction前に整合させます。active local / pending local / remote Transient / canonicalの違いとfailure・scope・session cleanupは[DIRECT_IP_RUNTIME.md](DIRECT_IP_RUNTIME.md#local-release-presentation-while-awaiting-authority)を参照してください。pendingは保存・snap・進捗に使用しません。

## Dirty同期とZ順序

### Local uncommitted rotation presentation

logical / predicted pose と drag translation は従来の4層です。CPU `PieceDataStore.states` はauthorityがcommitした
canonical poseです。`store.drag` はlocal active / pending Releaseのmembershipとscalar
translationです。`store.local_rotation` はlocal未ACK Rotate / RotateDragだけの疎な
pose overrideです。`RemoteDragPresentation` はremote Transientの平滑化されたtranslation
です。local回転をremote PlayerId slotへ登録しません。これらと独立した
`store.rotation_visual` の連続残差を GPU presentation で合成します。

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
走査/再構築せず、state/membership uploadは0 bytesです。wire 1、snapshot schema 1、
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

### Shared per-piece GPU metadata

`StateBuffers.piece_metadata` は次の3領域を1本の storage buffer に持つ SoA です。
`N` は GPU piece capacity です。drag liftは初回grab時だけ4番目のmappingとrecord tailを追加し、
CPUのlift mappingもその時点で遅延生成します。piece stateのAoS strideは変更しません。

```text
shared per-piece GPU metadata buffer (3N × u32)
  ├─ [0, N)   component roots
  ├─ [N, 2N)  remote drag slots
  └─ [2N, 3N) rotation animation slots (DSU root で参照)

first grab only:
  ├─ [3N, 4N) drag elevation slots
  └─ [4N, ...) 16-byte drag envelope records (shared storage binding)
```

byte base は root = 0、remote = 4N、rotation = 8N、drag lift = 12N、lift records = 16Nです。
Rust の `PieceMetadataLayout` が u64 で size / range offset を計算します。
全 metadata write（root / remote の初期 snapshot を含む）は checked range helper を使い、
`start <= capacity` と `len <= capacity - start` を満たさない場合は GPU write 前に拒否します。
範囲異常は panic せず `RenderReady` に epoch 付き renderer error を記録し、ready を解除します。
同じ epoch の upload / draw / picking を停止し、payload が正常に戻っても再開しません。
新しい epoch では buffer を再初期化して再開できます。初期化中のエラーは既存の
`GenerationError::Renderer` 経路で通知します。
検証は release build でも有効で、隣接領域への侵入を防ぎます。領域末尾の空 range は許可します。
WGSL は `config.capacity` に基づく `component_root` / `remote_slot` / `rotation_slot` で参照します。
新 epoch では `initial_roots` を先頭領域へ upload し、残りは wgpu の zero initialization
を利用します。remote の既存初期 snapshot は remote 領域だけへ適用します。
`root_revision` / `remote_revision` / `rotation_revision` と dirty range は独立しており、
1領域の更新は他の領域を再 upload しません。storage / copy-dst / copy-src を維持します。
100万 pieces では3領域合計12,000,000 bytes（約12 MB / 11.44 MiB）で、旧3本の合計と同じです。
512-byte remote delta uniform と可変長32-byte rotation record buffer は別に保持します。
初回drag後はlift mappingに4,000,000 bytesと小さなrecord poolを追加します。
CPUのdirty IDは125,000 bytes/100万pieceの`PieceBitSet`にまとめ、ID順から連続upload rangeを作ります。
grab境界でmemberごとのtree nodeは作りません。通常pointer frameはmapping / recordを再生成しません。
再grabのmaskは旧lift slotごとに分け、各partitionの現在値から独立したenvelopeを開始します。
単一partitionは元のmask Arcを共有し、混在時のID listは合計member数だけ保持します。DSUは参照しません。

storage binding 数は main / pick visibility compute がそれぞれ10→8です。draw / point /
rectangle の vertex は統合で9→7、さらに visibility を絞って6本です。
states / visible / drag_members / preview / piece_metadata / rotation_animations は vertex 限定、
selected は fragment 限定です。fragment は draw layout の selected 1本と selection layout の
selection / selectable 2本を全 render pipeline で共有し、合計3本です。
component preview compute は3本です。各 stage は8本以内ですが、
4本以下の downlevel limit や storage を使えない backend まで対応する変更ではありません。
通常 / far / picking の形状・presentation・画像 alpha の判定は共通のままです。

初回統合時（2026-10-06）の Windows / RTX 5090 / Vulkan（driver 610.88）の release 検証では、
device の storage binding 上限を8本に制限し、metadata の領域分離・sparse update・
epoch 再初期化、100万 pieces、normal / far / point / rectangle、continuous rotation の
DSU root history と通常 frame の upload 0、remote smoothing の mapping upload 0 を確認しました。
通常 workspace テスト891件と doctest 1件、Clippy、fmt が成功しました。
`gpu_` の ignored テスト（`gpu_million_selection_benchmark` と `gpu_remote_cursor_` を除外）は
26件中22件成功、4件失敗です。変更前 `f4e9acd` でも同じ4件が同じ結果で失敗し、25件中21件成功でした。
component preview の2 fixture は10,000座標の snapshot capture が `OutsidePlayArea(PieceId(0))`
となり、connected outline と rotated connected outline の2件は白を期待する色比較が
`[255, 250, 227, 255]` でした。これらの既存失敗は今回の検証では修正していません。

境界検証・visibility 変更後も同じ環境で、release の metadata 関連4件（うち実 GPU 1件）と
残る実 GPU 21件が成功しました。上記の既存失敗4件は除外しています。
通常 `jigsall-game` テスト725件と doctest 1件、workspace の Clippy、fmt も成功しました。

renderer error 化・quarter-turn 経路の追加後、同じ環境で通常 `jigsall-game` 727件と
doctest 1件、workspace 全 target の Clippy、fmt が成功しました。release の実 GPU 24件も
成功しました（上記の既存失敗4件を除外、既存の描画 benchmark を含む）。
5種類の不正 metadata range（root 初期値・root range・remote 初期値・remote range・rotation range）で、
隣接領域を変更せず renderer error を通知し、同 epoch の upload / draw を停止して次 epoch で
復旧することを確認しました。別 component が animation 中の非回転 piece について、
normal / far、四方向、非等方 pixel scale で idle 時と画像・point / rectangle picking が一致しました。

### Continuous rotation presentation

回転の責務は次の順です。

```text
canonical quarter-turn (PieceDataStore.states; authority が即時 commit)
  → prediction final pose (store.local_rotation; 未 ACK suffix)
  → continuous rotation record (store.rotation_visual)
      ├─ rigid residual transform
      └─ normalized elevation envelope (0..1)
  → render + GPU picking (presentation.wgsl の quarter-turn / presentation_pose)
```

`resources/rotation_visual.rs` は連続角度・pivot・補正 translation・開始時刻・duration・start_elevation を
component 単位で保持します。canonical rotation は 0 / 1 / 2 / 3 のままです。
90°あたりの初期値は `ROTATION_SECONDS_PER_QUARTER = 0.120` 秒で、`Time<Real>` を
First の time 更新後に取得します。virtual pause / simulation tick と独立した時刻です。
GPU と CPU 参照は同じ smoothstep easing を使い、final pose に対する残差を identity へ
戻します。個別 position の lerp は行わず、component 全体へ同じ剛体変換を適用します。
pivot は authority / prediction の `rotation_plan_with` が計算した AABB 中心を使います。

elevation は presentation-only の normalized scalar です。0は通常状態、1は回転中の最大追加 lift を
意味し、ゲーム上の高さや pixel offset ではありません。CPU `RotationAnimation::elevation(now)` と
WGSL `PresentationPose.elevation` が同じ cheap polynomial `s(t) = t²(3 - 2t)` を使います。
前半は start_elevation→peak、後半は peak→0 と補間し、開始・中間・終了の速度は0です。
peak は `max(start_elevation, clamp(abs(residual) / QUARTER_TURN, 0, 1))` から導出し、
保存しません。通常90°は0→1→0、小さい補正は残差角に応じた lift になります。
continuous helper は1回だけ求めた progress を位置・回転・elevation で共有します。

Q/E は現在の表示角度から signed / unwrapped な新 target へ retarget します。
animation queue は持ちません。prediction override の寿命と animation の寿命は独立です。
ACK で final world pose が変わらなければ残差の時間曲線を維持し、drag basis が変われば
pivot の base 座標を変換します。拒否・partial acceptance などで final pose が変わる場合は
現在の表示 pose から canonical / replay 後の pose へ剛体の残差を rebase します。
新 record の start_elevation は既存の Before capture で取得した現在の表示 elevation です。
Q/Q・E/E・Q/E、拒否・cancel・correction の境界でも値を引き継ぎ、終了時は厳密に0へ戻ります。
同じ final world pose の ACK は start / duration / start_elevation を含む envelope 全体を維持し、
elevation を reset / restart しません。追加 member scan や期限延長はありません。
local pointer delta は残差を適用した後に加算し、continuous angle を protocol basis や
drag delta に戻しません。release の final delta を反映してから残差を handoff します。
snap による component 結合・placement では対象の残差を破棄します。

`GpuRotationAnimation = 32 bytes/component` を維持し、末尾の padding を start_elevation に置換します。
新しい buffer / binding、per-piece memory は追加しません。
GPU は metadata の component root 領域 → `rotation_slot(root)` → 可変長 32-byte animation record
を参照します。stable minimum と DSU root が異なる union history にも対応します。
draw / main visibility / pick visibility は `rotation_active == 0` なら回転用の root / slot
を読みません。active frame でも `rotation_slot == 0` の piece は quarter-turn 専用経路を使います。
この経路の頂点変換は符号反転と xy 交換、AABB は偶奇による xy 交換です。
far splat の最小寸法は world-per-pixel の xy 交換と max で求め、sin / cos / length / sqrt
や animation progress / elevation curve / animation record load を評価しません。
非 animation piece の elevation は暗黙に0です。非等方な pixel scale でも draw と picking の footprint
を共有します。slot が非ゼロの piece だけが continuous pose / 回転 AABB / splat 計算を行います。
selection preview が active の場合の root 参照は、回転処理と独立して維持します。
固定 slot 上限はなく、独立 component はそれぞれ自身の pivot を持ちます。
rotation 用 GPU memory は metadata 内の root→slot 領域の 4N bytes と、同時 animation record の capacity × 32 bytes
（最低 1 record、増設時は 2 の冪）です。100万 pieces の rotation lookup 領域は 4,000,000 bytes です。
CPU は sparse な component record / slot map と upload 用 Arc を保持し、piece-sized な
恒久 animation / root mirror は作りません。操作境界で table と変更した root slot を upload
し、通常 frame は scalar clock と uniform だけ更新します。member scan、position 更新、
piece state / slot / record の再 upload はありません。終了 frame も追加 piece upload は不要です。
最後の animation が終わると uniform の active flag を落とし、shader の root lookup を省きます。
期限切れ record は次の変更境界で回収し、table 作成の時刻を原点にして GPU f32 時刻の精度を保ちます。

continuous 経路の通常 / far draw、main / pick visibility、point / rectangle は同じ `presentation_pose` を
使用し、continuous orientation に対応する AABB extent も共有します。
far splat は従来の pixel snapping と共通の footprint / alpha 判定を保ちます。
wire、save、snapshot、16-byte `GpuPieceState`、authority validation、snap / connectivity、
catch-up / migration は continuous presentation を参照しません。session scope 変更・終了、
Puzzle 初期化、snapshot / baseline install、Menu cleanup は animation を破棄します。
remote player の新規回転 animation はこの段階では開始せず、共通 transform と network から
独立した record を今後の入口として残します。elevation も同じ record で利用できます。
elevation は canonical / network / save / snapshot に存在せず、命令・authority・connectivity・
snap・physical / logical play area・Z-order にも含めません。本体の world / clip position、AABB、
picking geometry、SDF、UV、depth は elevation を使用しません。fragment varying も増やしません。
shadow 専用 vertex だけが elevation を screen-space separation に変換します。
side / thickness は elevation 非依存の静的な screen-space offset です。
top bevelは静的な表面補正、local / remote drag liftは独立した80ms envelopeとして実装しています。
selection単独のliftは未実装です。

### Pseudo-3D presentation

```text
quality + screen-space LOD
        ↓
resolved visual config
        ↓
shadow separation = base + max(rotation elevation, drag elevation) * extra lift
        ↓
static side / thickness = quality / LOD で決定、elevation 非依存
        ↓
top piece
```

`render/visuals.rs` のローカル `PieceVisualQuality` resource は Low / Medium / High を
`ResolvedPieceVisuals` に変換します。暫定 default は同じ箇所の High です。
extract は projected piece の短辺から shadow / side の独立した threshold を O(1) で解決します。
Low / LOD off の optional pass は pipeline を新規 queue せず、raster / draw を完全 skip します。
初めて必要になった frame で要求された optional pipeline をまとめて lazy queue し、共通 helper
`optional_render_pipelines` が準備状況を扱います。現在の epoch がまだ `RenderReady` でない初回表示は
要求された shadow / side が完成するまで待ちます。表示済み epoch では準備中の feature だけを skip し、
準備済み feature / top / picking を継続します。compile failure は従来どおり epoch の renderer error です。
100万 piece の far overview は全 quality で shadow / side draw 0、bevel無効です。
UI、Auto、設定保存、frame-time による動的調整は未実装です。

静止 piece は base shadow を持ち、animation slot が非ゼロの場合だけ既存の continuous pose の
elevation を使って追加 separation を加えます。local / remote dragの80ms smoothstep envelopeも独立に
評価し、rotationとのmaxだけをshadow separationへ使います。`elevation != thickness != bevel` です。
elevation が0に戻っても base shadow と side は残ります。side の厚みは Medium 1 px / High 1.5 pxで
回転開始・中間・終了とも一定です。`PSEUDO_3D_DIRECTION` と preset は `visuals.rs` に集約し、
side color は linear RGB の暗い neutral、opacity は現在1で source alpha を掛けます。
side導入時に既存 `PuzzleUniform` を32 bytes拡張しました。top bevelは未使用paddingへ4 scalarを収め、
336 bytesを維持します。buffer / storage binding / per-piece state は増やしません。

top bevelは既存top fragment内だけのstatic fake lightingです。Mediumは28 px以上で幅1 px、
Highは18 px以上で幅1.5 pxです。shadow / sideと独立してextractでO(1) resolveし、farでは常に無効です。
全辺SDFによるcoverageを維持し、selection / previewと共有する`outer_boundary_distance`で結合内部辺を
除外します。全4辺connected memberはfinite constantをderivativeへ渡してlightingをskipします。
外周distanceのscreen derivativeをdiscard前・frame uniform分岐内で求め、その長さでpixel距離へ変換します。
normalと共通方向の反対`-PSEUDO_3D_DIRECTION`の内積からlinear RGBを控えめに補正し、alphaを維持します。
光源はscreen-space左上に固定され、camera / quarter-turn / continuous rotationでも同じ向きです。
bevelの後に既存selection / preview outlineを適用します。elevationを幅・強度へ使いません。
Low / LOD off / farは高価なbevel計算をskipします。追加draw / pipeline variant / cull拡張はなく、
pick / shadow / side fragmentとtop vertexのquarter-turn fast pathは維持します。

描画順は main visibility / sort → selection preview → shadow depth / color → side → top → box です。
既存 visible IDs / indirect args / image texture を再利用し、追加 culling や CPU piece list はありません。
normal では同じ procedural profile / SDF / UV / source alpha、far では同じ quarter splat / center alpha を
使用します。world / piece の回転に影響されない右下への pixel offset を viewport サイズから clip-space に
変換するため、zoom しても距離は一定です。非 animation piece は既存 quarter-turn fast path を維持します。
top / shadow / side は literal boolean を渡す別 entrypoint です。top / picking は optional offset / color
計算や新しい varying を持ちません。side は同じ continuous position / angle を使い、elevation は読みません。
同じ offset の全 silhouette を先に描き、元位置の top union で覆うため、connected component の内部辺に
側面の線を作りません。side fragment は connected edge cache / component member を評価しません。

main visibility の `visual_cull_extent` は有効な shadow の最大 base + lift と side thickness の最大値を
inverse clip matrix で world-space に変換した保守的 extent です。side だけ有効でも画面端を保持します。
point / rectangle の ROI と raster は本体の bounds / geometry のままで、shadow / side だけの pixel は
選択できません。ゲーム状態・Z-order・authority・protocol・save は変更しません。

単純な alpha blend + depth write では、後方→前方に描いた shadow が積み重なります。
そのため既存 Depth32Float target を clear して silhouette の最前 depth を確定する prepass を行い、
color pass は source alpha × opacity の黒を一度だけ blend します。shadow 専用の depth は既存 rank の
順序を保って `[0, 0.5]` に収め、最大 loose Zでも補正の余地を残します。color fragment の depth を
正の float の1 ULPだけ進め、strict Greater test/write を使うことで、奥の shadow と同 rank の重複も
拒否します。この微小な変更は shadow 専用の一時 depth だけに適用します。
side は indirect draw 1回です。opaque は既存 Z rank の depth test / write と blend なしで前面を決めます。
translucent は既存の sorted visible IDs と alpha blend / depth writeなしを使い、新しい sort はありません。
side の開始時と top の開始時に既存 depth target を clear し、shadow の一時 depth を引き継ぎません。
top は従来の depth / ordering を維持します。
shadow が有効で pipeline が準備済みの frame は indirect draw 2回、無効または準備中なら0回です。
新しい texture / binding は不要です。
同 rank・異 alpha の shadow が完全に重なる場合は最初に通った silhouette の alpha を使います。

`shadow_draws` / `side_draws` は frame 単位で2 / 1回、無効または準備中なら0回です。
GPU diagnostic span は `puzzle_shadow` / `puzzle_side` です。dragのmembership変更を描画専用resourceで
監視し、release後も下降中のrecordを維持します。member走査・mapping uploadはcontrol境界だけです。
既存metadata bindingを初回grab時にだけ拡張し、lift用SoA mappingと16-byte record tailを共有します。
pool拡張はGPU copy、通常drag frameは既存uniformの時刻だけ更新します。idleのlookup/補間はgateで
skipし、追加draw・piece upload・O(N) CPU処理はありません。実 GPU fixture と短い release 計測は
[擬似3D描画](PSEUDO_3D.md)を参照してください。

2026-10-06、上記 RTX 5090 / Vulkan 環境で `procedural_gpu_benchmark` を直前の検証済み
`48f55f8` release build と比較しました。4096²画像・1024² offscreen・非 continuous rotation
で変更前後を交互に各3回実行し、各 run の30 frame平均 GPU timestamp の中央値を使いました。
100万 piece 全体表示（far）の結果は次のとおりです。

| mode | cull before → after (ms) | draw before → after (ms) |
| --- | --- | --- |
| opaque | 0.0272 → 0.0280 | 0.4950 → 0.4949 |
| translucent | 0.0182 → 0.0183 | 0.5080 → 0.5008 |

全 benchmark 条件で visible count は一致しました。この測定では明確な時間短縮は確認できず、
GPU / compiler / bottleneck に依存するため、演算の省略から速度向上率を推定しません。

### Remote drag presentation

Direct-IP hostとclientは、game-layerの`network/runtime/presentation.rs`で検証済みcontextを`RemoteDragPresentation`へ変換します。hostは`ProtocolDragContexts` / `HostCommandOutcome`、clientは`PeerReplicationState`と成功したauthority / Transient routeを使用します。rendererはnetwork runtime型に依存せず、offlineでも同じ空のresourceを使います。

`PieceDataStore`がcanonical position・ownership・rotation・Z・connectivityの正本です。canonical positionはReliable authority commitでだけ変わり、networkがacceptedした`RemoteDragUpdate.delta`はpresentationの最新targetです。GPUが使うdeltaはCPUで平滑化したdisplayed deltaで、表示位置はcanonical position + displayed deltaです。Transient / smoothing frameは`GpuPieceState`、snapshot、save、progress、snap、authority cursorを変更せず、smoothed valueを`PeerReplicationState` / `ProtocolDragContexts`へ戻しません。local active pointerとpending Releaseは従来の`store.drag.members` / GPU bitset / `PuzzleUniform.drag_delta`を使用し、local playerはremote slotへ登録せず即時feedbackを維持します。HELDによる選択除外とcanonical Zを維持します。

remoteはmetadataの`u32 remote drag slots[N]`領域（0=無し、1..64=slot）と512-byte固定delta uniformです。uniformは2つのVec2を1つのvec4にpackします。remoteのGPU常駐分は共有metadata内の4N + 512 bytes、100万pieceで4,000,512 bytes（約3.815 MiB）。CPU cacheは4N-byte mapping、dirty bitset（100万で125,000 bytes）、最大64のmembership bitsetとdeltaです。Dense accepted targetのbitsetはArc共有し、SparseはGrab境界でだけbitsetへ展開します。Sparseの新規bitsetは1 slotあたり最大125,000 bytes、全64 slotで最大8,000,000 bytesです。upload snapshot / rangesのpayloadは別途保持し、dense時は最大4N bytesです。PlayerIdはgame-layerの最大64件のslot lookupだけにあり、GPUはPlayerIdを検索しません。

Reliable `GrabAccepted`でauthorityのexact accepted membershipを割り当て、displayed / targetをcontextの現在deltaへ即時一致させます。partial acceptanceも要求targetではなく受理済みcontextを参照します。最初の空slotを再利用し、前playerのtarget・displayed・smoothing ageを引き継ぎません。Release / Cancelでは保存した正確なbitsetを使ってmappingを0にし、両deltaとsmoothing stateを直ちに破棄します。ReleaseのsnapでDSUが結合しても、旧membershipは変わりません。lagが残っていてもReliable canonical final positionへ即時handoffし、post-release animationは行いません。`DragRotationCommitted`ではmembershipを維持し、Reliable rebase後のnew-basis context deltaへdisplayed / targetを即時一致させます。旧basisから補間せず、旧basis / release / cancel後、duplicate / staleのTransientは既存benign-drop contractで落とし、targetを変更しません。

通常Transientはplayer→slot lookupと最新target更新だけでO(1)です。同frameのburstは最後のaccepted targetを上書きし、sample queueは作りません。Lastの`prepare_remote_drag_upload`は全network poll / command処理の後、epoch同期 → smoothing advance → upload準備の順で実行され、ExtractScheduleより前に同frameのtargetを表示へ反映します。`Time<Real>`のframe deltaを使うためvirtual pause / slow-motionに依存せず、wire tickを時間として解釈しません。

`resources/remote_drag.rs::REMOTE_DRAG_SMOOTHING`へ調整値を集約しています。`display += (target - display) * (1 - exp(-dt / tau))`、tau = 25 msで、約75 msで95%、100 msで98%追従します（固定targetに対する式の値）。各軸は現在displayとtargetの間へ収まり、direction reversalでもvelocityの慣性やovershootはありません。prediction / extrapolation / RTT補償 / clock同期は行わず、packet loss中も最後のtargetへ向かうだけです。各軸の残差が0.001 world units以下、またはtarget変更から250 ms経過したframeでexact settleします。250 ms以上のframe hitchは即時snapです。

target / displayed / smoothing ageと64-bit active maskはmappingから独立した再構成可能なCPU cacheです。追加CPU領域は固定1,032 bytes（targets 512 + age 512 + mask 8、paddingを除く）。idleはactive maskで即returnし、frame advanceは最大64 slotだけを見ます。piece / membership / `PieceDataStore.states`走査、mapping再構築はありません。Lastでdirty mappingだけを連続rangeにまとめ、最大128 spansを超えたらenclosing rangeを1回uploadします。smoothingが変えるのはdisplayed delta revisionだけで、active frameのcanonical / mapping / membership uploadは0 bytes、remote delta uploadは最大512 bytesです。settle後の次frameではdelta revisionも止まり、両remote uploadは0 bytesです。membershipのO(N)処理はReliable control / initialization境界に限ります。piece Entity / Meshは追加しません。

client ReadyではJoinBaseline / catch-up / FinalDragSet reconciliationが完了した**current** replica contextからmembershipを構築し、displayed == target == reconciled deltaへ即時初期化します。初回Transientを待たず、過去のdragをzeroからanimationさせず、final scalar rollbackもそのまま表示します。store epoch / authority scopeの変更、join baseline / new session、snapshot / new puzzle、Menu / session stop / host lossでmapping・membership・dirty ranges・両delta・smoothing stateをresetし、GPU revisionを進めます。renderer bufferはpiece epochとともに作り直し、remote mapping / deltaのrevisionが一致した後に描画・RenderReadyを進めます。

`presentation.wgsl::presentation_pose`はmain visibility、pick ROI visibility、normal / far-splat vertexに共通です。その内部の`presentation_position`がdrag translationを合成します。point / rectangleは同じvertexを使います。canonical HELDを前提にlocal membershipを優先し、remote translationを重ねて二重移動させません。wire v1、`GpuPieceState` 16 bytes、snapshot schema 1、join baseline schema 1は変更しません。

接続componentのselection / preview outlineは、dense stateのflags bit 5–8にあるtop / right / bottom / leftの接続cacheを使って内部辺を除外します。cacheはDSUの派生情報で、既存snap closureのneighbor探索内で両側をincrementalに更新し、変化したpieceだけdirtyにします。16-byte stateを維持し、snapshot schema 1のinstallでは復元DSUからcacheを再構成します。fragmentは4辺SDFを一度だけ計算し、coverage / pickingは全辺、黄 / 青outlineは共通の未接続境界を使います。全4辺が接続した内部pieceにoutlineはありません。

rectangleはselectableなdirect hitだけをmaskへrasterし、preview中だけ1回のGPU computeでcomponent rootのmaskへcollapseします。component atomicなauthority更新とvalidated restoreにより、正規状態のselectabilityはcomponent内で揃います。main vertexがpreview中だけrootとpreview maskを読み、結果のPREVIEW bitを既存のflat flagsでfragmentへ渡します。root用varyingは追加せず、pick用uniformはpreview_activeを0にしてselection rasterのroot参照も避けます。final readbackは従来のdirect hit bitsetで、CPUのcommit_selectionがcomponent全体を再検証・確定します。GPU root領域は共有metadataの先頭4 bytes / pieceで、preview collapseには先頭のroot領域だけをbindingし、その長さをroot capacityとして扱います。CPUにはroot dirty bitsetだけを持ち、unionでabsorbed memberをdirtyにして最終rootを先頭領域へrange uploadします。initial / restore時だけDSUから全rootを生成します。idle / camera / pointer dragでroot scan・root upload・preview computeはなく、rotation / previewが両方inactiveならvertexもrootを参照しません。pipelineとメモリ・計算量は[GPU_PICKING.md](GPU_PICKING.md)に記載しています。

Core2d main transparent pass後のカスタムpassです。背景画像Spriteは通常Bevy描画。GPUは拡張quad AABBでvisible IDとindirect argsを生成し、topはdraw_indirect1回で、quality / 独立LOD が有効で準備済みの場合だけ、その前にshadow depth / colorとsideを追加します。4頂点はvertex_indexから作り、vertexで4辺を2 u32ずつ生成してflat varyingへ渡します。fragmentはSDF・画像alphaでdiscardし、UV・outlineを評価します。

opaqueは任意のinstance順でdepth test/write、半透明は可視IDだけをGPU radix sort（8bit × 3 pass）で後方→前方に並べblendし、depthを書きません。透明経路ではID順に可視IDを圧縮してから安定sortし、同じZのID順も維持します。workgroup数はGPUのinstance_countからindirect dispatchで決め、CPU readbackは不要です。matrix・state・visibleをpickingにも共有します。矩形overlayは追加draw1回です。sortは[TRANSPARENT_RADIX_SORT.md](TRANSPARENT_RADIX_SORT.md)、選択は[GPU_PICKING.md](GPU_PICKING.md)に記載しています。

## Persistent save の境界

ローカル進捗保存は `PieceDataStore / PuzzleDefinition → PuzzleCheckpoint → PuzzleSave → SaveCodec → SaveRepository → SaveStorage → FilesystemStorage` の流れです。画像は選択時の original encoded bytes を SHA-256 で識別し、再エンコードしない `.puzimg` を save 間で共有します。進捗 `.puzsave` v1 は 16 bytes/piece の明示的 little-endian codec と全体 / header checksum を使い、ランダム SaveId で保存します。新規ゲームにはUUID v4のGameIdを発行し、すべての手動保存・オートセーブとそのロードで引き継ぎます。オートセーブは同じGameIdの最新指定件数（既定1件）を保持し、新しい保存の成功後に古い履歴を削除します。最大510 bytesの header とファイル長だけで一覧とローテーション対象を判別し、非 authority の placed_count cache は完全 load 時に state と照合します。旧 format の互換コードは持たず、対応 version は1だけです。更新 request は読み込んだ revision を保持し、現在の header と不一致なら Conflict として publish 前に拒否します。ユーザータイトルは validation を持つ metadata で、filename / identity には使いません。

`GameSnapshot` schema 1は16-byte piece layoutを保持し、borrowed checkpoint view を通じて同じ capture / validation / install を使います。restore は DSU / 接続 GPU cache / placed_count を再構築し、GameData の progress / completion を同期します。load worker が準備した store を直接採用するため、random 初期配置は生成しません。既存 epoch / RenderReady による GPU 準備待ちの後だけ Playing / GameComplete へ遷移します。disk I/O、decode、codec は worker/channel に分離し、O(N) capture は明示 Save 時だけです。通常 play に新しい piece 数比例の処理や per-piece Entity / persistent Vec は追加しません。

保存先は OS user application data 以下で、logical key を storage に渡します。画像を先に保存し、save は temporary file の sync と atomic replace で publish します。通常 Save は共有画像の存在だけを確認し、import / load で画像全体の hash を検証します。SaveStorage は Send / Sync を要求せず、filesystem は worker で動かします。メニューのサムネイルは専用queue / workerと独立したfilesystem handleで読み込み・検証・decodeし、保存・ロードのworkerを占有しません。ImageHashで共有する最大64件の成功texture cacheをメニュー退出・session変更後も保持します。将来の Steam Cloud は handle を所有 thread に保持し、両workerからのStorageRequestsのoperationを非同期 API に dispatch、callback から返信する executor を追加します。StorageProxy を使う repository / codec / restore 準備は worker 上で継続します。write は encoded Vec の所有権を移譲し、proxy による全 blob コピーを避けます。Steam Cloud 自体は未実装です。形式・layout・failure / worker lifecycle の詳細は [PERSISTENCE.md](PERSISTENCE.md) を参照してください。

FilesystemStorage は Rust 標準のファイルロックで複数プロセスを協調させます。画像ごとの共有ロックを `ImageLease` として取り込み・ロードの応答から `OriginalPuzzleImage` へ移し、元 encoded bytes の解放後も使用中の画像を保持します。保存待ちの request も lease を共有し、画像の置き換え・session cleanup・古い応答の破棄で解放します。save に未参照の画像を掃除する場合は画像の排他ロックを待たずに試し、使用中なら見送ります。共通の repository 排他ロックは画像 import、save の revision 検証から公開、autosave ローテーション、delete の参照確認から掃除までを直列化します。専用 lock file は削除・置換せず、取得の再試行待ちは worker だけで行い、StorageProxy / executor には非 blocking の試行を渡します。プロセス間の直接通信はありません。

## Multiplayerの境界と課題

PieceIdはEntity IDから独立したu32、PlayerIdはu64です。version、seed、grid、画像寸法で形状を再構成します。core/sessionはsession identity・画像hash・命令sequence・authority epoch・migrationを、game/multiplayerはsnapshotの検証・復元とplayer単位の保持解放を提供します。GrabGroup / ReleaseGroupも同じ認証済みplayerとreliable control streamを使います。selectionはlocal presentationでありsnapshotには入りません。Direct-IP transport、途中参加、レート制限、Host / Join UIは[DIRECT_IP_RUNTIME.md](DIRECT_IP_RUNTIME.md)を参照してください。Host UIはCPU storeとGPUの準備後にlistenし、Join UIは専用接続画面でNetworkStatusを表示します。

transport向けにはcore/protocolのComponentRef / PieceTarget / ProtocolPieceCommandを使用します。minimum memberとexpected sizeでcomponentを参照し、32 componentまでcompact、より多いselectionは対象componentのcount / topology digest付きDenseへ切り替えます。game/multiplayer/protocolのopt-in authority adapterがcurrent connectivity・所有権・placed・enabledを再検証し、Grabで受理した結果だけをSparse / Denseのplayer別contextへ保持します。Denseはcomponent listへ展開せずcanonical bitsetを保持し、受理したmembershipをGrabAccepted ACK / authority event型で返します。既存Move sequenceはmembership不要のbest-effort DragUpdateにも共用し、ReleaseはGrab sequenceとfinal deltaだけで確定します。local PieceCommand / PieceBitSetとGPU経路は維持します。詳細は[MULTIPLAYER_PROTOCOL.md](MULTIPLAYER_PROTOCOL.md)を参照してください。

snapshot schema 1は16-byte stateのflagsへPLACED / CONNECTED_RIGHT / CONNECTED_DOWN / rotation（bit 9–10）を保存します。root IDはprotocolへ保存せず、install時に隣接edgeからDSUを再構成します。schemaの番号が1以外、境界外edge、placedの誤座標、component内のrotation / rigid transform / placed不一致は変更前に拒否します。初期のschema 1を含め、開発中のlayoutとの互換decoderはありません。restoreはpositions / Z / connectivity / placedを保ち、holds / selection / dragをresetします。disconnectはcomponent全体のholdだけを解放し、位置とsnapを変更しません。

GPUは描画と選択の補助で、placed・所有権・snapを決めません。cullingはO(N)、半透明sortは可視数に比例します。選択保持・drag pointer・rectangle previewのCPU処理はO(1)ですが、final selectionの再検証、grab時のZ順保持、release時のsnapとstate commitには明示的な大量処理が残ります。極端な重なりではrasterとpickingの負荷が増えます。異OS/GPU、通常windowの全手動操作は今後の確認対象です。
