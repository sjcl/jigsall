# 90°単位のcomponent回転

新規ゲームの「ピースの回転を有効にする」トグルで選択します。既定はオフです。
`PuzzleConfig.rotation_enabled`を開始時に`PuzzleDefinition.rotation_enabled`へ固定し、
ロード・joinでは保存済み／ホストのdefinitionを使用します。オンではseedの上下32bitと
row-major PieceIdを独立した整数hash domainへ入れ、初期rotationを0 / 1 / 2 / 3から
決定します。同じdefinitionで初期位置と向きを再構成できます。形状generatorはv1（開発時v5）です。
位置shuffleの乱数列は回転生成に消費しません。長方形を90°回した場合にも初期quadが
重ならないよう、オンでは長辺を一辺とするsquare slotを使います。カメラの初期表示範囲と
LogicalPlayAreaにも同じslot寸法を使います。初期state生成は従来のworkerで行います。

オフでは初期rotationは全て0です。回転キーの命令・hover pickを発行せず、HUDから
回転操作の案内を省きます。authorityはUIと独立して`Rotate` / `RotateDrag`を拒否し、
0回転／4回転の要求も許可しません。拒否時に位置・rotation・hold・drag delta / basisを
変更しません。local predictionとreplicaの回転commitも同じゲームルールで拒否します。
保存／snapshotのvalidationでは、オフのdefinitionに非ゼロrotationが含まれた場合を拒否します。

save formatはv1、snapshot schemaは1、wireはv1です。開発中のsave / wireとの
互換性は提供せず、snapshot schemaの番号が1以外も拒否します。各pieceのstateは16 bytesを維持します。

Q / Eで選択中のcomponentを反時計回り / 時計回りに90°回転します。
選択がない場合はカーソル下のピース、またはその結合済みcomponent全体を回転します。
複数componentはそれぞれ自身の現在position AABB中心をpivotにします。
ドラッグ中も同じGrabを維持して回転できます。PendingPoint / BoxSelecting中は無視し、
mouse releaseとQ/Eが同frameならReleaseを優先します。通常のRotateはunheld component用です。
ドラッグ中のRotateDragは受理した対象全体を検証し、1componentでも不正なら
stateとdrag basisを変更しません。placed、disabled、HELD / owner不一致、stale topology、
混在rotation、不整合なrigid transformを拒否します。自動snapは行いません。

## Cursor target without selection

Idleかつ選択が空のとき、Q / Eの押下時だけ既存のGPU Point requestを発行します。
クリックと同じ1画素のdepth / SDF / alpha / selectable判定で手前のピースを選び、
4-byte readbackを非同期で受け取ります。CPUの全piece検索、常時hover判定、
新しいGPU buffer / passは追加しません。通常pointer frameは早期returnします。

結果のPieceIdから`ComponentRef::from_member`で直接`PieceTarget::Component`を作るため、
component membershipや選択bitsetを展開しません。選択・hold・Zを変更せず、
既存Rotateのauthorityでcomponent全体の適格性を再検証して確定します。

同じ座標で結果待ち中にQ / Eを再入力した場合はturnsを合算し、同じrequestを再利用します。
異なる座標での新しい入力は最新requestを優先します。クリックでgestureが始まった場合、
選択が生まれた場合、focus喪失・UI capture・cursor無効の場合はpending回転を取り消します。
request IDが異なる遅延結果、empty hit、readback errorは回転を起こしません。

## Authoritative invariant

```text
rotation(C) ∈ {0, 1, 2, 3}
position(p) ~= rotate_quarter(correct_position(p), rotation(C)) + translation(C)
translation(C) = position(minimum_member(C))
               - rotate_quarter(correct_position(minimum_member(C)), rotation(C))
```

`rotate_quarter(v, 1) = (-v.y, v.x)`はworld座標の反時計回りです。
canonical quarter-turn はCPU/GPUとも符号反転とxy交換だけを使います。
continuous presentation の残差だけは sin / cos を使います。
`matches_transform`は従来と同じf32の加減算丸め許容を使い、edge driftを
snap epsilonとして許可しません。`matches_translation`はrotation 0の薄いwrapperです。

回転操作だけmemberを走査してworld AABB中心と新しいcanonical boundsを求めます。
中心とtranslationはf64で計算し、各positionをcanonical座標から再構築します。
すでに丸めたpositionを繰り返し回さず、singletonの中心はbitsまで保持します。
一時planは操作終了時に破棄します。continuous presentation の短命な component 残差は
canonical storage と独立して保持します。

## Drag rotation and rebase

```text
displayed position = committed position + drag.delta
Q / E:
    displayed AABB中心をcomponentごとのpivotにする
    新rotationのcanonical座標から最終positionを直接再構築する
    各stateを一度だけ更新する
    drag.delta = ZERO
    accepted result後にpointer anchor = current pointer
pointer move:
    drag.delta = current pointer - new anchor
```

`PieceCommand::RotateDrag { members, delta, quarter_turns }`は全対象をpreflightして
から一括適用します。singletonの新centerは元のposition + deltaそのもので、
delta ZEROならtiny value / signed zeroを含め元のbitsを保持します。表示位置が
非有限になる場合も全体拒否します。HELD / owner / Z / selection / membership /
connected edge bits / connectivity / component root dirtyは変更しません。
`apply_piece_commands`で成功した結果にだけanchor更新を適用し、その後Last scheduleで
stateとzero deltaを同時にupload準備するため、古いstate + zero deltaのframeを作りません。

## Snap / rendering / persistence

neighbor候補とclosureは同rotationの正しいgrid neighborだけです。
translation比較、stable minimum memberによるtie、固定final transformのclosure、
union順、rounded logical translation cache、境界scanの再利用を維持します。
boardはrotation 0 / translation ZEROであり、0°だけsnapします。
placed memberのrotation bitsは0です。Release中にrotationは変更しません。

CPU/GPU dense stateとsnapshot recordはともに16 bytesのままです。
共通helperがflags bit 9–10をencode / decodeします。
CONNECTED_TOP / RIGHT / BOTTOM / LEFTはcanonical edgeのcacheで、回転時に変更しません。
rendererはworld quadだけ回転し、SDF / UV / profile / outlineはcanonical localを使います。
visibilityとpick visibilityは共通の表示 orientation からAABB extentを求めます。
far splatも長辺を回転し、pixel-center snapping、alpha、depth、pick ROIを維持します。

snapshot schema 1はdefinitionにrotation_enabledを含み、16-byte recordでrotationを保存します。
validationはrotation統一、剛体変換、placedのrotation 0と正解positionを検証し、
install時のconnection cacheはDSUから再構築します。ローカルsave codecも同じ
flagsを保存・検証します。

`ProtocolPieceCommand::Rotate`と`RotationCommitted`は既存PieceTarget / ComponentRef /
DenseTargetを使うreliable controlです。受信側は全accepted componentを事前検証し、
同じ決定的計算を適用してrotationを含むresult fingerprintを照合します。
相違は既存のDiverged / trusted snapshot resync経路で処理します。

ドラッグ中のprotocolはmembershipを再送しない
`RotateDrag { grab_sequence, final_delta, through_tick, quarter_turns }`と
`DragRotationCommitted`です。`through_tick: Option<u64>`は回転前に送信した最大tickで、
まだTransientを送っていない場合はNoneです。final_deltaだけで欠落した更新を補えます。
成功時に同じcontextのdeltaをZERO、tick floorをthrough_tickへ更新します。
ticksはgesture全体で単調増加し、floor以前の更新を再適用しません。

Moveのafter_control_sequenceとRemoteDragUpdateのbasis_sequenceは、初めはGrab、
rebase後は成功したRotateDragのcontrol番号です。grab_sequenceはgesture終了まで同じです。
古いbasisの遅延packetを拒否し、新basisのpacketがreliable回転より先に来た場合も
誤適用せず拒否します。次の最新Transientまたはreliable操作のfinal_deltaで補えます。
拒否されたRotateDragはcontrol番号だけを消費し、前のbasis / tick / deltaは保持します。
fingerprintは対象state・rotation・hold・connectivityに加えcontextのGrab / basis / tick /
zero deltaも検証します。transportは現行のwire version 1のみをdecodeし、互換decoderはありません。

## Cost and verification

2026-10-04、初期回転モードの追加後に`cargo test --workspace --locked`はdoctest込み797件、
`cargo test --workspace --locked --all-features`はGNS localhost / doctest込み815件が通過しました。
全target / 全featureのClippy（`-D warnings`）とfmtも通過しました。
GNSのMSVC検索先は[Windowsビルド手順](WINDOWS_BUILD.md)に従い、そのコマンドの`LIB`に
生成済み`out/lib`を補いました。共有Cargo cacheとvcpkg作業パスは変更していません。
追加回帰はseed両半分の固定vector、長方形scatterの非重複とcamera範囲、開始時のrule固定、
日英UIのトグル操作、無効時のキー／local command／wire request／replica commit拒否、
拒否後のdrag継続、save／snapshotのmode保持と不正rotation・旧形式の拒否を検証します。
Windowsのrelease実GPUテスト`gpu_quarter_turn_images_shapes_and_picking_agree`も通過し、
四方向の画像・形状・point / rectangle pickingの一致を確認しました。

canonical quarter-turn の通常frame処理はGPUのrotation bit decodeと必要なxy交換です。
continuous presentation 中の追加処理は以下の節を参照してください。
`DragTransform { members, delta }`、O(1) pointer更新、dirty range upload、
component root buffer、idle CPU処理は既存の構造を維持します。
pointer dragはCPU O(1)、state upload 0、membership upload 0のままです。
drag rotationは明示的なQ/E時だけO(k)で、rebase後のpointer処理は再びO(1)です。
回転時は対象memberだけdirtyにし、fragmentedなdirty範囲も隙間を含めずuploadします。
network clientの未ACK回転だけは疎なlocal pose overrideを保持します。authorityと同じ
plannerをcanonical + predicted pose viewへ適用し、controlのcommitted prefixだけを退役して
残るsuffixを再生します。Q/Qの最初のACKでも表示は180°を維持します。RotateDragは
protocol basis / anchorを先行更新せず、predictionが消費したdeltaを表示baseから差し引き、
scalar drag.deltaと合成します。Release待ちも同じsuffixを維持します。詳細・atomicな
schedule順・GPU restore・4層の責務は[ARCHITECTURE.md](ARCHITECTURE.md#local-uncommitted-rotation-presentation)
と[Direct-IP runtime](DIRECT_IP_RUNTIME.md#local-release-presentation-while-awaiting-authority)を参照してください。

## Continuous visual rotation

```text
canonical quarter-turn → prediction final pose
  → continuous rotation record
      ├─ rigid residual transform
      └─ normalized elevation envelope (0..1)
  → GPU presentation → render + picking
```

Q/E のゲーム上の90°回転はこれまでどおり即時確定します。画面上だけ、最終 pose に対する
残差角（通常 -90° / +90°）を約120msで0へ補間します。定数は
`resources/rotation_visual.rs::ROTATION_SECONDS_PER_QUARTER` に集約し、`Time<Real>` と
smoothstep の ease-in-out を使います。連打時は現在の表示角から新 target へ retarget し、
残差の角距離に応じた時間を使います。Q / E の signed な方向を保つため、
270°→0°で逆方向へ270°回転しません。個別 piece の position lerp は使いません。
authority と prediction が共有する `rotation_plan_with` の pivot に対する剛体回転です。

rotation record は presentation-only の elevation も保持します。0は通常の平面状態、
1は回転中の最大 lift を表す normalized value であり、ゲーム上の高さではありません。
CPU `RotationAnimation::elevation(now)` と WGSL `PresentationPose.elevation` で取得でき、
位置・回転・elevation は GPU continuous helper の1回の progress 計算から導出します。
前半は start_elevation→peak、後半は peak→0 を `s(t) = t²(3 - 2t)` で補間し、
開始・中間・終了の速度は0です。通常90°は開始0・中間1・終了0です。
peak は `max(start_elevation, clamp(abs(residual) / QUARTER_TURN, 0, 1))` から導出します。
45°補正の angular peak は0.5で、既に高い場合は現在値以上を保ちます。
新 animation は既存の Before capture で得た表示中の elevation を start_elevation に引き継ぎ、
Q/Q・E/E・Q/E、拒否・cancel・rebase で突然0へ落としません。追加の member scan はありません。

`store.local_rotation` は未 ACK 回転の最終 pose override のままです。
`store.rotation_visual` が別の animation lifetime を持ち、30ms の ACK で override が
消えても animation は終了しません。ACK による basis 変更は同じ time curve を維持して
start / duration / start_elevation を含む elevation envelope も reset / restart せず、
残差を rebase します。拒否・partial acceptance は現在の表示から新しい final pose へ戻し、
終了時は追加 transform が identity になり、progress >= 1 では elevation は厳密に0です。
elevation のために寿命を延ばさず、
期限切れ record は既存の変更境界で回収します。通常の Rotate と RotateDrag は同じ record
と shader を使い、pointer の translation は残差の後に加算します。
release / cancel の logical pose 変更も操作境界で rebase し、snap による topology 変更や
placement は残差を破棄します。session teardown / scope 変更、Puzzle 変更、snapshot /
baseline install では全 animation を破棄します。

GPU root buffer を使う root→slot lookup は4 bytes / piece、`GpuRotationAnimation = 32 bytes/component`
です。末尾 padding を start_elevation に置き換え、buffer / binding や per-piece memory は増やしません。
slot 数は可変で、RotateDrag の独立 component は別々の pivot を持ちます。
CPU は操作境界でだけ member planner / table 準備を行い、animation frame は clock / uniform
を更新するだけです。CPU position の全 member 再計算と `GpuPieceState` 再 upload はありません。
main / pick visibility、normal / far draw、point / rectangle は quarter-turn と continuous の
2経路を共有します。`rotation_active == 0` では回転用 root / slot lookup を省き、active frame
でも slot が0の piece は quarter-turn 経路です。符号反転・xy交換で頂点と AABB を求め、
far splat の最小寸法も pixel scale のxy交換とmaxで計算し、sin / cos / length / sqrt を使いません。
slot が非ゼロの場合だけ `presentation_pose` と連続回転の footprint を評価します。
非 animation piece は elevation が暗黙に0で、slot==0 の経路では elevation curve・progress・
animation record load を追加しません。
UV / shape / alpha は canonical local の共通判定を保ちます。

continuous angle / progress は save、snapshot、protocol、authority、snap、connectivity、
gameplay validation に含めません。remote player の新規回転 animation は未対応です。
elevation は canonical / network / save / snapshot に存在せず、PieceCommand・authority・
connectivity・snap・physical / logical play area・Z-order にも加えません。world / clip position・
AABB・picking geometry・SDF・UV・depth は elevation をまだ使用しません。fragment varying も増やしません。
shadow、side / thickness、bevel、lighting、pixel offset 変換、graphics quality、screen-space LOD、
drag / selection の通常 lift は後続です。同じ elevation を次の effect の入口に使えます。
GPU layout と lifetime の詳細は
[アーキテクチャ](ARCHITECTURE.md#continuous-rotation-presentation)を参照してください。

追加 CPU 回帰は開始 / 途中 / 終了、剛体性、identity、Q/Q・E/E・Q/E、wraparound、
30ms ACK と prediction retirement、拒否、drag pointer / ACK basis、release final delta、
reset、DSU root history、256独立対象、100万 pieces の frame 中 piece access なしを扱います。
network runtime 回帰は実際の ACK route / suffix replay を通して通常回転と RotateDrag を検証します。
実 GPU fixture は normal / far の45°途中に main draw / point / rectangle が一致し、
animation frame の state / root / animation table upload が0であることを検査します。

elevation の CPU 回帰は0→1→0、45° / 10° / translation-only residual、0..1の範囲、
開始・中間・終了の速度、Q/Q・E/E・Q/Eの値の連続性、非ゼロ start_elevation を持つ ACK basis
rebase、拒否・cancel の引き継ぎと exact settle、32-byte layout と期限切れ回収を検査します。
network runtime の early ACK 回帰でも envelope を比較します。shader source 回帰は progress の
共有、binding 数、varying なし、slot==0 の fast path を検査します。
実 GPU の `gpu_rotation_elevation_matches_cpu_envelope_and_record_layout` は残差角・開始 elevation・
時刻の460 sampleで CPU/GPU を比較します。既存 normal / far fixture は同じ時刻で elevation
だけを変え、全 pixel・可視 ID・point / rectangle の結果が不変であることも検査します。

```powershell
cargo test --workspace --locked
cargo test --locked -p jigsall-game --release gpu_continuous_rotation -- --ignored --nocapture --test-threads=1
cargo test --locked -p jigsall-game --release rotation_visual_tests -- --ignored --nocapture --test-threads=1
```

2026-10-06、elevation 追加の基準 `54835ea` に対して、この Windows ホストで workspace 通常テスト
898件（doctest込み）、全 target / 全 feature の Clippy（`-D warnings`）、fmt が通過しました。
release の rotation visual 実 GPU テスト4件が通過し、460 sample の CPU/GPU elevation 比較と
progress >= 1 の exact zero、normal / far の全 pixel・可視 ID・point / rectangle の不変性、
別 component の animation 中の quarter-turn fast path、animation frame の upload 0を確認しました。
速度 benchmark と他 OS / GPU の検証は今回再実施していません。

2026-10-06、Windows / NVIDIA GeForce RTX 5090 / Vulkan（NVIDIA 610.88）で確認しました。
workspace 通常テストは doctest 込み869件が通過し、全 target / 全 feature の Clippy
（`-D warnings`）と `cargo fmt --all --check` も通過しました。
release の GPU 回帰21件と、追加の細長い far splat fixture 1件、合計22件が通過しました。
新規2 fixture は release で個別にも実行しました。45°途中の剛体 position / orientation、
normal / far の main draw と point / rectangle の一致、DSU root history、細長い splat の
実際の回転 footprint、animation frame の state / root / animation table upload 0 bytes、
終了時の identity を検証しました。既存の100万 pieces coverage / picking 回帰も通過しました。
上記2026-10-04の記録で baseline でも失敗した4 fixture は今回の GPU 回帰から除外しました。
速度 benchmark、FPS、他 OS の実機検証は行っていません。

```powershell
cargo test --workspace --lib
cargo test -p jigsall-game --release --locked --lib render::tests::rotation_tests -- --ignored --test-threads=1
```

CPUテストはsingletonの4000回転、fractional gridの剛体再構成、pair / L字、
独立pivot、hold / topology拒否、rotation-aware snap / closure / drag、
checkpoint / save / snapshot round-trip、reliable authority / replica / wireを検証します。
実GPU fixtureは4方向の色付きUVとSDF、point / rectangle picking、内部edge outlineの
抑制、非正方形AABB、viewport端のfar splatを検証します。drag rotationのfixtureは
回転時の16-byte state upload、membership / root upload 0、回転後pointer frameの
全upload 0とRelease後のpickingを検証します。

local未ACK回転の回帰はCPU 11件と実GPU 1件を追加しました。通常Q/Q・Q/Q/E、
pointer A → Q → pointer B → Q → pointer C → Releaseを扱い、authority ACKを別frameで
1件ずつ配送します。各frameのworld position / orientationとLastのuploadを検査し、
accepted poseの巻き戻りを検出します。partial Grab（ACK前Releaseも含む）とpartial
Rotate、snapshot / checkpointのcanonical分離、古いgesture token、reject / cancel /
send / protocol / disconnect / scope / Menuのcleanupも検証します。1M-piece回帰では
small / dense回転、canonical Arcの非COW、affected range restore、pending idle frameの
piece access / state upload 0を検査します。実GPU回帰はnormal / far描画、culling、
point / rectangle picking、HELD / Z維持、16-byte uploadとoverride解除を確認します。

2026-10-04、baseline `247cba28339c04c965afe5250e1fabd3990d9c07` に対して
`cargo test --workspace --release --locked` はdoctest込み746件、
`cargo test --workspace --locked --all-features` はGNS localhost / doctest込み763件が通過しました。
全target / 全featureのClippy（`-D warnings`）とfmtも通過しました。
Windows / NVIDIA GeForce RTX 5090 / Vulkanで、次の実GPU選択実行は追加回帰を含む17件が
通過し、既存4件が失敗しました。

```powershell
cargo test --workspace --release --locked gpu_ -- --ignored --skip benchmark --nocapture --test-threads=1
```

失敗は`gpu_component_preview_crosses_mask_words_and_preserves_direct_high_bits`と
`gpu_component_rectangle_preview_matches_final_selection_without_readback`のfixtureによる
`OutsidePlayArea(PieceId(0))`、`gpu_connected_selection_outlines_preserve_coverage_picking_and_uploads`と
`gpu_rotated_connected_outline_keeps_canonical_internal_edges_hidden`の内部edge色比較です。
変更前baselineのclean worktreeでも4件すべて同じ失敗を再現しました。今回の回帰とは分けて
記録し、既存fixture / outlineは変更していません。速度benchmark・他OSの実機検証は未実施です。

drag rebaseのCPU回帰は、singleton / pair / fractional L字の繰り返し回転、独立pivot、
anchor成功時更新と拒否時維持、同frame Release優先、fragmented memberのexact upload、
snap遅延、同rotation neighbor / 0°board、同Grab内の複数rebase、欠落Transientの補完、
古い / future basisとfloor以前の更新拒否、Sparse / Denseの事前検証、DSU root historyの
相違、fingerprint mismatchとDivergedを含みます。開発時のwire v2のgolden fixtureは10件です。

カーソル下回転のCPU回帰は単体 / component、選択優先、key合算、stale結果、
クリック・focus / UI / cursorによる取消、遅延中の適格性変更、pointer-only frameの
request / upload 0を検証します。実GPUでは重なりの手前のsingletonとその下の
結合済みcomponentをそれぞれQ / Eで回転し、state uploadが16 / 32 bytes、
membership / root uploadが0、選択・holdが変わらないことを確認します。
session local playerを42にして両経路を検証しました。2026-10-03、最新版masterとの
統合後にworkspace通常テスト370件とdoctest 1件、追加の実GPU1件が通過しました。
workspace全targetのClippy（CPU reference / picking features、`-D warnings`）と
`cargo fmt --all --check`も通過しました。

通常回転の初版では2026-10-03、Windows / NVIDIA GeForce RTX 5090（Vulkan）でworkspace通常テスト
295件とdoctest 1件、既存を含む実GPU回帰15件が通過しました。
`cargo fmt --all --check`とworkspace全targetのClippy（CPU reference / picking
features有効、`-D warnings`）も通過しました。`--all-features`の検査は
GNS native buildのlibclang不足で未完了です。CPU/GPUの速度benchmarkは再計測していません。
