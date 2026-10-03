# Host Migration 基盤

## Baseline と変更範囲

作業開始時のHEAD: `7d85e57a3ac5e6375f192b1f698e036f77200849`
(`fix: smooth procedural tab roots with analytic fillets`)。

このHEADから専用worktreeとbranch `codex/host-migration-foundation` を作成した。
初回実装では元checkoutのgenerator v5化とshape/renderer系の並行作業を保持した。
その後、`master` の `4fec6dc5fa3a960d69ad7ffe866fcacc3014f07c`
(`perf: move multi-drag transforms and selection previews to GPU`) へrebaseした。
現在はmasterのgenerator v5とGPU drag/preview基盤を含む。
snapshotは版番号を固定せず、現在の `PuzzleDefinition.validate()` に委譲する。
host migrationの変更は既存GPU hot pathやgeneratorへ追加処理を接続しない。

追加APIはopt-inで、single-playerのBevy scheduleや入力経路には接続していない。
通常frameに全件走査、追加upload、host判定は発生しない。
Steamworks、socket、packet framing、実際のnetwork serializationは実装していない。

## Session と authority

`core/src/session.rs` がtransport非依存のpure logicを持つ。

| 型 | 責務 |
| --- | --- |
| `SessionId(u128)` | ゲームsessionの識別。backendが新sessionに一意なIDを割り当てる |
| `ImageHash([u8; 32])` | 保持するencoded画像bytesのSHA-256。画像の内容識別 |
| `SessionDefinition` | SessionIdと必須image_hash。AuthoritySession生成時に固定 |
| `AuthorityEpoch(u64)` | host authorityの世代 |
| `AuthoritySequence(u64)` | その世代で適用済みのauthority変更番号 |
| `AuthorityCursor` | epoch、sequenceの辞書順比較 |
| `ClientCommandEnvelope` | session、epoch、player、stream別ClientCommandSequence、PieceCommand |
| `AuthorityEventEnvelope<T>` | session、host、cursor、将来のauthority event payload |
| `ClientCommandSequence` | reliable Control sequence、またはunreliable Move tickと参照control |
| `CommandSequenceTracker` | player別にcontrolとMoveを独立検証 |
| `AuthoritySession` / `MigrationState` | 稼働、正常transfer、recoveryの状態遷移 |
| `RecoverySource` | snapshot提供peerと適用済みcursor |

`PlayerId` は従来のbackend非依存identityのまま。
SteamIDからPlayerIdへの対応は将来のSessionBackendが管理する。
envelopeのplayer/hostフィールドだけでは認証にならない。
backendはcommand送信者、event送信者、snapshot提供者、owner変更通知を認証する。

`AuthoritySession::new()` はSessionDefinitionを必須とし、image_hashを省略しない。
同じsession内で画像は変更せず、host migration後も同じ定義を保持する。
画像を差し替える場合は新SessionIdでsessionを作る。
`ImageHash` の対象は元hostが保持・配信するencoded画像bytesそのもの。
ファイル名、URL、画像寸法、GPU textureや各clientで再encodeした画像のhashではない。
この基盤は32-byte digestの保持と一致検証を行う。画像bytesからのSHA-256計算と
参加時/受信時の実bytes照合は次フェーズのbackend adapterが行う。

AuthoritySequenceはcommand sequenceと別物。
世代のbaselineはsequence 0で、hostが変更を確定すると
`advance_authority()` で1ずつ進む。migration後は必ずepoch + 1、sequence 0。
counterはchecked arithmeticで扱い、上限では拒否してwrapしない。

`PieceDataStore.epoch` はローカルGPU buffers/readback/uploadのライフサイクル。
network cursorには使わず、snapshot install時は既存のローカルatomic allocatorで
新しい値を割り当てる。同一network epochでも各clientのGPU epochは異なり得る。

## Command と event の検証

既存のBevy `ClientCommand` と `PieceCommand` は変更していない。

`AuthoritySession.accept_command()` はmigration中のcommandを `Frozen` で拒否し、
稼働中は `CommandSequenceTracker.validate_and_record()` を呼ぶ。
playerごとに以下の2streamを独立して保持する。Moveはcontrolのcounterを進めない。

| command | metadata | 配送と検証 |
| --- | --- | --- |
| Grab / Release | `Control(sequence)` | reliable、0から連続。duplicate/stale/gapを拒否 |
| Move | `Move { after_control_sequence, tick }` | unreliable、参照control内でtickを検証。gapは許容 |

controlに欠番があれば `ControlGap { expected }` を返し、counterを変更しない。
backendはreliable controlの順序を保証するか、未受理commandを保持して欠番補完後に再試行する。
既に受理したcontrolは再適用しない。Moveの先着で後続controlがStaleCommandになることはない。

Moveの `after_control_sequence` は、clientが最後に送信したcontrol番号を表す。
hostがその番号までcontrolを消費していなければ `ControlNotProcessed { required }` で拒否する。
現在のcontrol番号より古ければ `StaleMoveContext` で拒否する。
参照controlが一致した場合だけMove tickのduplicate/staleを拒否し、
forward gapは `Gap { expected }` として検出して受理する。
Move tickは同じplayer・参照control内のcommandごとに0から増やす番号で、
同frameに複数pieceをMoveするときも別tickを割り当てる。
controlを1つ受理するたびMove tickをresetする。
既に送信済みの古いMoveを新しい参照controlで再ラベルして再送してはいけない。

例:

~~~text
Move(after=0, tick=100) → Grab(0)前なので拒否、counterは不変
Grab Control(0)        → 受理
Move(after=0, tick=100) → 受理、Move gapを許容
Release Control(1)     → Moveのtickと無関係に受理
Grab Control(2)        → 受理
遅延Move(after=0, 101) → 前回dragなので拒否
Move(after=2, tick=0)   → 新しいcontrol文脈として受理
~~~

これはplayerごとの順序境界であり、host発行leaseや所有権の証明ではない。
他pieceのcontrolを送った場合も番号を進めるため、旧番号のin-flight Moveは捨てる。
所有権、PieceId、有限座標、placedへの操作可否は引き続き `apply_piece_command()` が検証する。
trackerを通した後にgameplayが拒否したcontrolも番号を消費する。
host adapterはenvelope受理後にgameplayの適用/拒否を同期的に確定してから次を処理し、
queued commandだけで「controlを処理済み」と判断しないこと。

session不一致とepoch不一致（古い世代・未通知の未来世代とも）は両streamで拒否する。
commandとstreamの不一致（MoveをControlに載せる等）も拒否する。
拒否はどちらのcounterも変更しない。migration完了時は両streamをresetする。
trackerのメモリはplayer数に比例し、per-piece stateや通常single-player frameの処理は追加しない。

`validate_event()` は稼働中にsession、epoch、host、次のauthority sequenceを検証する。
duplicate/stale event、gap、未来epochを拒否する。
gameplay適用成功後に `record_applied_event()` でcursorを進める。
これはreliable authority変更用の順序境界であり、Move全フレームのevent sourcingではない。
gapを見つけたbackendはretransmissionまたはsnapshot resyncを要求する。

streamを分離しても、最後のMove欠落時にReleaseが最終位置を確定する要件は残る。
下記TODOのReleasePiece { id, final_position }と実transportの配送・ACKは次フェーズで実装する。

## Snapshot

`game/src/multiplayer/snapshot.rs` のschema version 3（永続連結を追加しversion 1 / 2は拒否）:

| フィールド | 内容 |
| --- | --- |
| `schema_version` | 形式の版番号 |
| `session` | SessionId |
| `image_hash` | sessionで合意したencoded画像bytesのSHA-256 |
| `cursor` | snapshotが反映済みのAuthorityCursor |
| `definition` | generator version、seed、grid、画像寸法、snap距離 |
| `next_z_order` | 次のfront操作に使用するZ番号 |
| `pieces` | row-majorの `Vec<SnapshotPieceState>` |

`SnapshotPieceState` は `Vec2 position + u32 z_order + u32 flags` の16 bytes。
snapshot flagは独立した `SNAPSHOT_PLACED`、`SNAPSHOT_CONNECTED_RIGHT`、`SNAPSHOT_CONNECTED_DOWN`を許可する。
右・下のgrid edgeからDSUを再構成し、root IDは保存しない。詳細は[CONNECTED_SNAPPING.md](CONNECTED_SNAPPING.md)。
`GpuPieceState` をwire形式としてserializeしない。
100万件のpiece payloadはメモリ上で16 MB（serializerのencoded sizeは未規定）。
per-piece Entity、Mesh、Handle、String、HashMapは追加しない。

意図的に含めない状態:

- selected / preview flagsとその集合
- held flag、holder identity、drag gestureとGPU表示用のmembers/delta
- ENABLED（現状全ピース共通なのでinstall時に再構築）
- dirty IDs、GPU epoch、highlight caches、GPU readback
- placed_count / progress（snapshotのPLACEDから再計算）
- 画像bytesやGPU texture

`GameSnapshot::capture()` は通常 Save と同じ `PuzzleCheckpoint::capture()` を使い、
source storeを変更せず、確定済みの canonical state のみを保存する。
active local / remote drag があってもcaptureでき、migration / recoveryはDragの終了を待たない。
表示位置が `states.position + drag.delta` でも保存するのは `states.position` だけで、
deltaを加算せず、captureのためにcancel / Release / snapしない。
hold / holder identity / active drag context / transient deltaはserializeせず、restore時に破棄する。
`RotateDrag` / `DragRotationCommitted` でcommit / rebase済みの位置と回転は保存し、
その後の未確定移動だけを捨てる。Grab時にcanonical Zがfrontへ更新済みならそのZも保存する。
Grab開始前の完全なstateへのrollback履歴は持たない。
`SnapshotExpectation` はsession/recovery交渉と現在のPuzzleDefinitionから作る期待値。
image_hashはsession開始時に合意したSessionDefinitionから取得する。
`install_migration_snapshot()` もAuthoritySessionのimage_hashを使い、受信snapshotの
自己申告hashを期待値として採用しない。同じ画像寸法でも別hashなら復元を拒否する。
snapshot自身から期待値を決めてvalidationを迂回してはいけない。

install前にschema、session、image_hash、cursorの完全一致、PuzzleDefinition.validate()、
期待するdefinitionとの完全一致、piece count、全座標の有限性、
next_z_order（piece_count以上、MAX_Z以下）、各z_order（next_z_order未満）、
許可flag、境界外の連結flagがないこと、placedの正解座標との完全一致、component内のoffset / placed一致を検証する。
definitionはpiece_count計算より前に検証し、壊れたgridでもpanicを起こさない。
invalid snapshotはResultで拒否し、storeを変更しない。

installはdense statesとconnectivity、placed_count / next_z_orderを復元し、
store内のhold、selection、dragのmembers/delta、以前のhighlight caches、dirty IDsを全て消す。
GPU側のbox previewなど別resourceのcleanupは、下記TODOのbackend adapterが担当する。
placedとpositionとrotationとz_orderはそのまま保存する。migration時にsnapは実行しない。
新しいローカルepochを割り当てるので、既存 `prepare_piece_upload()` の次回実行が
full initial uploadを行い、古いGPU readbackは無効になる。
通常のその次のidle frameでは全量uploadもdirty rangeも発生しない。

`ProtocolDragContexts` / `PeerReplicationState` はsession、authority epoch、store generationで
contextをscopeする。installはstore generationを更新するため、外部から直接restoreしても
旧contextは読めず、次のcommand/event/update適用時に破棄される。
`PeerReplicationState::install_snapshot()` は成功時に直ちにcontextをclearする。
migration後の旧epochのDragUpdate / Release / RotateDragおよび対応eventは、
canonical state変更前のepoch検証で拒否される。

capture / validate / installは稀なcheckpoint処理としてO(N)。
join、reconnect、host migration、明示的checkpointでのみ呼び出す。
画像とPuzzleDefinition resourceは呼び出し側が既に保持している前提で、
installは一致を要求し、画像resourceを置き換えない。

canonical stateのcutは、完了したcommand適用の間で取得する。通常Saveはmain worldの
PostUpdateで `apply_piece_commands` 後に実行され、`Res<PieceDataStore>` のread borrowと
command側の `ResMut<PieceDataStore>` が同時変更を防ぐ。multiplayerはscheduled systemを
追加しない同期APIで、`ProtocolDragContexts::apply_replicated()` はgameplay変更とcursor更新を
完了してから戻る。adapterはその呼び出し間でstoreとlast applied cursorを組にしてcaptureする。
peerでも `apply_event()` が完了してcursorを記録した境界を使う。Drag完了待ちやtimeoutは不要。

## Graceful migration

例: Aがhost、B/Cがclient、cursor 3:100。

1. SessionBackendが次host候補Bを外部で決める。
2. runtime adapterが新規入力を停止し、実行中のauthority command適用を完了する。
   active dragをfinish / Releaseする必要はなく、playerのDrag終了を待たない。
3. `begin_graceful(B)` でauthorityとcommand処理を凍結する。
   この例の最終cursor 3:100は最後に適用済みのcanonical stateに対応する値。
4. `GameSnapshot::capture()` でfinal snapshotを作り、Bへ転送する。
5. Bが信頼済みexpectationで `snapshot.validate()` を行い、検証済みsnapshotを保持してACKする。
   この時点ではstoreへinstallしない。
6. `acknowledge_snapshot(session, B, final_cursor)` でACKを記録する。
   別session、別player、別cursorのACKは拒否する。
7. 将来のSteam SetLobbyOwner後、認証済み通知から `host_changed(B)` を呼ぶ。
   ACK前のowner変更や別候補への変更はgraceful transitionでは拒否する。
8. `install_migration_snapshot()` がsnapshotと遷移を検証し、ここで初めて一度だけ
   dense stateを変換・installして全holdを解除し、B / epoch 4 / sequence 0として稼働を再開する。
9. Aは退出し、Cも同じcursorと新authorityへ移行する。

ACK前のvalidationはO(N)の検証走査だけで、dense stateの変換・割当やstore/GPU epochの変更は行わない。
保持したsnapshotはACK後に変更せず、最終install時にも再検証する。
検証はACK前とinstall前の2回、CPU側のsnapshot restoreはowner移行後の1回となる。

最終helperはsessionのcopyでtransitionを事前検証してからstoreをinstallし、
成功時だけ新sessionをpublishする。validation失敗やepoch上限でstore/sessionは変化しない。
coreの `complete_migration()` はpure transitionだけを担うため、
ゲーム側ではsnapshotと一緒に処理するこのhelperを使用する。
backendはsourceとcursorの通知を各peerに伝え、同じ交渉結果を再現する必要がある。

## Abrupt loss と recovery source

1. `host_lost()` で直ちに凍結する。正常transfer途中のhost lossもこの経路へ切り替える。
   runtime adapterはpeer自身の未確定local drag/predictionを停止する。
   capture前に `drag.members` / `drag.delta` をclearする必要はなく、
   restore時に破棄する。deltaをcanonical stateへcommitしない。
   現在のfinishや通常cancelの確定経路はMove/Releaseを生成するため、この破棄には使わない。
2. Steam Lobby等の外部backendが新ownerを選び、`host_changed(new_host)` を通知する。
   owner通知が先に来た場合、Activeから直接recoveryへ入ることもできる。
3. peerのsession / player / last applied cursorを収集する。
4. `choose_recovery_source()` で同session・失われたepochの候補に限定して選ぶ。
   cursor最大、同cursorなら最小PlayerIdが勝つ。入力順序によらない。
5. source peerはactive dragを残したまま、最後に受信・適用済みのcanonical dense stateから
   `GameSnapshot::capture()` する。選んだsourceからsnapshotを受け、
   認証済みsourceと完全一致するcursorを確認する。
6. `install_migration_snapshot()` で復元し、epoch + 1 / sequence 0で再開する。

B=4:801、C=4:805、D=4:805ならCがsnapshot source。
新hostは外部通知でCでもBでもよく、source選定はhost選出ではない。
`select_recovery_source()` 自体は同session内のcursorをepoch/sequenceの辞書順で比較する。
migration stateは失われたepoch以外をさらに除外し、未来epochへの飛び越しを認めない。
選択候補の最大cursorがローカル適用済みcursorより古ければ拒否し、凍結状態を維持する。
例: local=4:806、候補最大=4:805は拒否。候補が揃うまで待つかローカルsnapshotを提供する。
complete_migration()でも同じ下限を再検証し、選択済みsourceと一致しても巻き戻しを許可しない。
この協力ゲームの信頼前提で、同じcursorまたはより新しいcursorのsnapshotを復旧の正本として使い、全holdを解放する。
移行後、古いhostのcommand/eventはepoch不一致で拒否される。

## Recovery の信頼モデル

この基盤は参加peerを信頼する協力ゲーム向けで、host-authoritativeを
悪意ある参加者に対するセキュリティ境界として扱わない。
通常操作の決定権とcommand検証はhostに集約するが、abrupt recoveryで集めるcursorと
snapshot内容はpeerの自己申告。例としてCが4:805を名乗り、形式検証を満たす状態を
提供すれば、cursorの下限などの条件を満たす限り採用できる。
source本人の認証、cursor一致、形式検証は、旧hostがその内容を確定した証明にはならない。
image_hashも画像の同一性を確認するメタデータであり、snapshot全体の真正性を証明しない。
peerが同じhashを申告して異なる画像bytesを送るケースは、backendでの実bytes照合が必要。

将来、悪意あるpeerへの防御を必要とする場合は、旧host発行checkpointの検証を必須にする。
checkpointはsession、cursor、PuzzleDefinition、image_hash、dense stateなどに結び付いた
canonical snapshot hashを持ち、旧hostから認証済み経路で事前配信・cacheするか、
署名と検証keyの管理を行う。提供peerが自己申告するsnapshot hashだけでは不足する。
install前に内容のhashを再計算して照合し、検証できるcheckpointがない場合は復旧を拒否する。
その設計では、旧hostが確定していても証明をpeerへ未配信の最新状態は復旧できない。
今回の信頼モデルではこのcheckpoint認証を実装しない。

## 通常client disconnect

`release_player_holds(&mut store, player) -> Vec<PieceId>` を使用する。
該当playerのholdをcomponent全体へ展開して取り除き、HELDを消し、dirty IDを追加する。
戻り値はPieceId昇順。position、placed、z_order、next_z_orderと他playerのholdは維持する。
再度呼び出しても変更しない。snapや配置eventは発生させない。
計算量はbitsetとhold / component member数に依存し、全piece stateは走査しない。
正常状態では他playerのcomponentを維持する。矛盾した混合ownerが既にある場合は当該componentの全holdを解放する。
backendはmembership変更をreliableに伝え、このhelperで確定した解放をpeerへ反映する。

## Local simulation tests

Steamもsocketも使わないテストを追加した。

- core: per-player duplicate/stale、control gap拒否、Move gap許容、wrong session、古い/未来epoch。
- core: channel順序逆転、Move先着後のGrab/Release受理、control参照の未来/古さ、stream不一致、両counterの上限。
- core: graceful freeze、ACKのplayer/session/cursor検証、外部owner一致、sequence reset、session画像identityの維持。
- core: abrupt freeze、source選択、決定論的tie-break、epochの辞書順比較、local=4:806で最大4:805の候補拒否、completion時の巻き戻し防御。
- core: eventのhost/session/epoch/sequence検証、counter上限、graceful中の突然切断。
- game: A→B gracefulとCの復元、ACK前のvalidationでstore/GPU epochを維持しuploadを起こさないこと、
  owner移行後の復元、position/placed/Z/next_z維持、hold/drag/highlight cleanup、
  placed_count再計算、実際のPieceUpload full upload、次idleの空upload、再Grab。
- game: abrupt B=4:801/C=D=4:805、epoch 5:0と旧epoch command/eventの拒否。
- game: Bが10/11/25を保持した状態から切断、他playerの12は維持、dirty IDs。
- game: active local drag（membersあり、delta=(100, 200)）中のcanonical capture成功とstore全状態の維持。
- game: rotation rebase後のtransient deltaを持つhost / remote peer snapshotの一致とrestore時のdrag破棄。
- game: migration後の旧epochのDragUpdate / Release / RotateDrag拒否とcanonical stateの維持。
- game: wrong session/image hash/count、NaN、±Infinity、unsupported generator/schema（旧version 1を含む）、
  古い/未来cursor、異なるdefinition、不正grid/Z/flagsをResultで原子的に拒否。
- game: snapshot検証失敗時はauthorityを有効にしない。image_hash不一致でもauthority/store/GPU epochを変更しない。
- game: 実際のPieceStateでMove先着後もReleaseでき、再Grab後に前回dragの遅延Moveを拒否する。
- game: 16-byte dense snapshotの100万ピースexport/install round trip。

検証コマンド（masterへrebaseした専用worktree内）:

~~~sh
cargo fmt --all --check
cargo check --workspace --locked
cargo clippy --workspace --locked --all-targets --all-features -- -D warnings
cargo test --workspace --locked
~~~

Cargo.toml / Cargo.lockを変更しない。
renderer / selection / shader / GPU state layout / generator / benchmarkは変更しない。
既存の入力、snap、session lifecycle、dense stateをworkspace testsで引き続き検証する。
GPU/CPU benchmarkは再実行しない。実GPUが必要な既存4件とCPU benchmark 1件はdefaultでignored。

masterへのrebase後の実行結果: 上記4コマンドは全て成功。workspace testsは71件成功、5件ignored。
新規テストはcore 12件 + game 8件。Cargo.lockの開始時/終了時SHA256は
`D52C7FBFE817D6178A851C4BA09A6D66AD48EEF105F476CFA6E1B08CD23B4458` で一致した。
検証のworkspace成果物は専用worktree内のtargetへ分離し、外部dependencyの既存cacheだけを再利用した。source変更は専用worktreeに限定した。

## Steam integration の接続点とTODO

- SteamID→PlayerId、SessionId割当、membershipと送信者の認証。
- Lobby作成/参加/Owner callback/SetLobbyOwner。host選出はSteam backendの責務。
- snapshot transfer、ACK、cursor収集、recovery交渉結果のpeer配信。
- Steam Networking Sockets / SDR、packet framing、encoded schemaとサイズ上限。
  decoderはschema/definitionを検証し、piece_countに応じて割当を制限する。
  現在のtyped validationは過大packetをdeserializeする前のDoS対策ではない。
- reliable Grab / Release / Snap / PuzzleDefinition / Snapshot / HostChanged / membership。
- unreliable Moveはhost loss直前の数フレームが失われることを許容する。
  正確なdrag再現や全Moveのevent sourcingは行わない。
- **Release自身で最終位置を確定するwire commandが必要。**
  例: ReleasePiece { id, final_position }。
  最後のunreliable Moveがlostしても古い座標でrelease/snapしないこと。
  今回のlocal PieceCommand::Release APIは変更しない。
- compressed image bytesの保持、SHA-256計算、session参加時と画像受信時の実bytes照合、
  新hostから途中参加者への画像転送。image_hashの型・必須メタデータ・snapshotとの
  一致検証は実装済み。今回は参加済みclientが同じ画像を既に持つ前提。
- reconnect timeout、再試行、ownerがrecovery途中で再び消失した場合の交渉再開、
  重複callbackの吸収、欠落eventの再送/resync。
- runtime adapterでcommand適用境界のcaptureと、migration / recovery時の入力停止・
  restore後のprediction破棄を接続する。captureはactive drag中も可能で、自動commitはしない。
- runtime wiring時はgesture、pending selection/readback、overlay、queued command/event、
  GameData progress/completionを同期してから入力を再開する。
  PieceDataStoreのpresentation stateは既にresetするが、これらの別resourceのcleanupは
  次フェーズのbackend adapterが担当する。
- lost hostの最新checkpointをpeerが十分な頻度で保持する仕組み。
  trust policyは上記の参加peer信頼。セキュリティ境界へ変更する場合は旧hostの
  checkpoint真正性を検証する別設計が必要。
- cross-platform identity、latency/scoring、persistent saveはこのフェーズの範囲外。
