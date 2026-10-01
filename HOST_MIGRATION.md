# Host Migration 基盤

## Baseline と変更範囲

作業開始時のHEAD: `7d85e57a3ac5e6375f192b1f698e036f77200849`
(`fix: smooth procedural tab roots with analytic fillets`)。

このHEADから専用worktreeとbranch `codex/host-migration-foundation` を作成した。
元checkoutにはgameplayのgenerator v5化とshape/renderer系の未コミット変更が
存在したため、それらを保持し、取り込んだり戻したりしていない。
このbranchのgeneratorはbaselineのv4のまま。
snapshotは版番号を固定せず、現在の `PuzzleDefinition.validate()` に委譲する。
並行作業が取り込まれたビルドではそのビルドの対応版を検証する。

追加APIはopt-inで、single-playerのBevy scheduleや入力経路には接続していない。
通常frameに全件走査、追加upload、host判定は発生しない。
Steamworks、socket、packet framing、実際のnetwork serializationは実装していない。

## Session と authority

`core/src/session.rs` がtransport非依存のpure logicを持つ。

| 型 | 責務 |
| --- | --- |
| `SessionId(u128)` | ゲームsessionの識別。backendが新sessionに一意なIDを割り当てる |
| `AuthorityEpoch(u64)` | host authorityの世代 |
| `AuthoritySequence(u64)` | その世代で適用済みのauthority変更番号 |
| `AuthorityCursor` | epoch、sequenceの辞書順比較 |
| `ClientCommandEnvelope` | session、epoch、player、player別command sequence、PieceCommand |
| `AuthorityEventEnvelope<T>` | session、host、cursor、将来のauthority event payload |
| `CommandSequenceTracker` | player別のsequence high-water mark |
| `AuthoritySession` / `MigrationState` | 稼働、正常transfer、recoveryの状態遷移 |
| `RecoverySource` | snapshot提供peerと適用済みcursor |

`PlayerId` は従来のbackend非依存identityのまま。
SteamIDからPlayerIdへの対応は将来のSessionBackendが管理する。
envelopeのplayer/hostフィールドだけでは認証にならない。
backendはcommand送信者、event送信者、snapshot提供者、owner変更通知を認証する。

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

- session不一致とepoch不一致（古い世代・未通知の未来世代とも）を拒否する。
- 同playerの直前と同じsequenceをduplicate、それより小さい値をstaleとして拒否する。
- playerごとにsequence 0から開始する。migration完了時にtrackerを作り直す。
- forward gapは `Gap { expected }` として検出し、受理する。
  unreliable Moveの欠落を許容するためである。
- 拒否時はhigh-water markを変えない。

trackerは認証済み入力のreplay検証であり、所有権、PieceId、有限座標、
placedへの操作可否は引き続き `apply_piece_command()` が検証する。
trackerを通した後にgameplayが拒否したcommandもsequenceは消費する。
transportによる再送で同じcommandが後から別の結果になることを防ぐ。

`validate_event()` は稼働中にsession、epoch、host、次のauthority sequenceを検証する。
duplicate/stale event、gap、未来epochを拒否する。
gameplay適用成功後に `record_applied_event()` でcursorを進める。
これはreliable authority変更用の順序境界であり、Move全フレームのevent sourcingではない。
gapを見つけたbackendはretransmissionまたはsnapshot resyncを要求する。

将来、reliable actionとunreliable Moveを併用する場合は、古いreliable actionが
新しいMoveのhigh-water markに追い越されない配送・sequence設計が必要。
現在のglobal player sequence trackerだけで混在channelの信頼性は保証しない。

## Snapshot

`game/src/multiplayer/snapshot.rs` のschema version 1:

| フィールド | 内容 |
| --- | --- |
| `schema_version` | 形式の版番号 |
| `session` | SessionId |
| `cursor` | snapshotが反映済みのAuthorityCursor |
| `definition` | generator version、seed、grid、画像寸法、snap距離 |
| `next_z_order` | 次のfront操作に使用するZ番号 |
| `pieces` | row-majorの `Vec<SnapshotPieceState>` |

`SnapshotPieceState` は `Vec2 position + u32 z_order + u32 flags` の16 bytes。
snapshot flagは独立した `SNAPSHOT_PLACED` だけを許可する。
`GpuPieceState` をwire形式としてserializeしない。
100万件のpiece payloadはメモリ上で16 MB（serializerのencoded sizeは未規定）。
per-piece Entity、Mesh、Handle、String、HashMapは追加しない。

意図的に含めない状態:

- selected / preview flagsとその集合
- held flag、holder identity、drag gesture
- ENABLED（現状全ピース共通なのでinstall時に再構築）
- dirty IDs、GPU epoch、highlight caches、GPU readback
- placed_count / progress（snapshotのPLACEDから再計算）
- 画像bytesやGPU texture

`GameSnapshot::capture()` はsource storeを変更せず、presentation flagsを除いて保存する。
`SnapshotExpectation` はsession/recovery交渉と現在のPuzzleDefinitionから作る信頼済みの期待値。
snapshot自身から期待値を決めてvalidationを迂回してはいけない。

install前にschema、session、cursorの完全一致、PuzzleDefinition.validate()、
期待するdefinitionとの完全一致、piece count、全座標の有限性、
next_z_order（piece_count以上、MAX_Z以下）、各z_order（next_z_order未満）、
許可flagだけであることを検証する。
definitionはpiece_count計算より前に検証し、壊れたgridでもpanicを起こさない。
invalid snapshotはResultで拒否し、storeを変更しない。

installはdense statesとplaced_count / next_z_orderを復元し、
hold、selection、preview、以前のhighlight caches、dirty IDsを全て消す。
placedとpositionとz_orderはそのまま保存する。migration時にsnapは実行しない。
新しいローカルepochを割り当てるので、既存 `prepare_piece_upload()` の次回実行が
full initial uploadを行い、古いGPU readbackは無効になる。
通常のその次のidle frameでは全量uploadもdirty rangeも発生しない。

capture / validate / installは稀なcheckpoint処理としてO(N)。
join、reconnect、host migration、明示的checkpointでのみ呼び出す。
画像とPuzzleDefinition resourceは呼び出し側が既に保持している前提で、
installは一致を要求し、画像resourceを置き換えない。

## Graceful migration

例: Aがhost、B/Cがclient、cursor 3:100。

1. SessionBackendが次host候補Bを外部で決める。
2. `begin_graceful(B)` でauthorityとcommand処理を凍結する。
   最終cursorは3:100に固定される。
3. `GameSnapshot::capture()` でfinal snapshotを作り、Bへ転送する。
4. Bが信頼済みexpectationでvalidate/installしてからACKする。
5. `acknowledge_snapshot(session, B, final_cursor)` でACKを記録する。
   別session、別player、別cursorのACKは拒否する。
6. 将来のSteam SetLobbyOwner後、認証済み通知から `host_changed(B)` を呼ぶ。
   ACK前のowner変更や別候補への変更はgraceful transitionでは拒否する。
7. `install_migration_snapshot()` がsnapshotと遷移を検証し、全holdを解除して復元し、
   B / epoch 4 / sequence 0として稼働を再開する。
8. Aは退出し、Cも同じcursorと新authorityへ移行する。

最終helperはsessionのcopyでtransitionを事前検証してからstoreをinstallし、
成功時だけ新sessionをpublishする。validation失敗やepoch上限でstore/sessionは変化しない。
coreの `complete_migration()` はpure transitionだけを担うため、
ゲーム側ではsnapshotと一緒に処理するこのhelperを使用する。
backendはsourceとcursorの通知を各peerに伝え、同じ交渉結果を再現する必要がある。

## Abrupt loss と recovery source

1. `host_lost()` で直ちに凍結する。正常transfer途中のhost lossもこの経路へ切り替える。
2. Steam Lobby等の外部backendが新ownerを選び、`host_changed(new_host)` を通知する。
   owner通知が先に来た場合、Activeから直接recoveryへ入ることもできる。
3. peerのsession / player / last applied cursorを収集する。
4. `choose_recovery_source()` で同session・失われたepochの候補に限定して選ぶ。
   cursor最大、同cursorなら最小PlayerIdが勝つ。入力順序によらない。
5. 選んだsourceからsnapshotを受け、認証済みsourceと完全一致するcursorを確認する。
6. `install_migration_snapshot()` で復元し、epoch + 1 / sequence 0で再開する。

B=4:801、C=4:805、D=4:805ならCがsnapshot source。
新hostは外部通知でCでもBでもよく、source選定はhost選出ではない。
`select_recovery_source()` 自体は同session内のcursorをepoch/sequenceの辞書順で比較する。
migration stateは失われたepoch以外をさらに除外し、未来epochへの飛び越しを認めない。
選択候補の最大cursorがローカル適用済みcursorより古ければ拒否し、凍結状態を維持する。
例: local=4:806、候補最大=4:805は拒否。候補が揃うまで待つかローカルsnapshotを提供する。
complete_migration()でも同じ下限を再検証し、選択済みsourceと一致しても巻き戻しを許可しない。
同じcursorまたはより新しいcursorのsnapshotを正本として使い、全holdを解放する。
移行後、古いhostのcommand/eventはepoch不一致で拒否される。

## 通常client disconnect

`release_player_holds(&mut store, player) -> Vec<PieceId>` を使用する。
sparse held_byから該当playerだけを取り除き、HELDを消し、dirty IDを追加する。
戻り値はPieceId昇順。position、placed、z_order、next_z_orderと他playerのholdは維持する。
再度呼び出しても変更しない。snapや配置eventは発生させない。
計算量は全ピース数ではなくhold数の走査と、返すIDのsortに依存する。
backendはmembership変更をreliableに伝え、このhelperで確定した解放をpeerへ反映する。

## Local simulation tests

Steamもsocketも使わないテストを追加した。

- core: per-player duplicate/stale/gap、wrong session、古い/未来epoch。
- core: graceful freeze、ACKのplayer/session/cursor検証、外部owner一致、sequence reset。
- core: abrupt freeze、source選択、決定論的tie-break、epochの辞書順比較、local=4:806で最大4:805の候補拒否、completion時の巻き戻し防御。
- core: eventのhost/session/epoch/sequence検証、counter上限、graceful中の突然切断。
- game: A→B gracefulとCの復元、position/placed/Z/next_z維持、hold/highlight cleanup、
  placed_count再計算、実際のPieceUpload full upload、次idleの空upload、再Grab。
- game: abrupt B=4:801/C=D=4:805、epoch 5:0と旧epoch command/eventの拒否。
- game: Bが10/11/25を保持した状態から切断、他playerの12は維持、dirty IDs。
- game: wrong session/count、NaN、±Infinity、unsupported generator/schema、
  古い/未来cursor、異なるdefinition、不正grid/Z/flagsをResultで原子的に拒否。
- game: snapshot検証失敗時はauthorityを有効にしない。
- game: 16-byte dense snapshotの100万ピースexport/install round trip。

検証コマンド（baseline worktree内）:

~~~sh
cargo fmt --all --check
cargo check --workspace --locked
cargo clippy --workspace --locked --all-targets --all-features -- -D warnings
cargo test --workspace --locked
~~~

Cargo.toml / Cargo.lockを変更しない。
renderer / selection / shader / GPU state layout / generator / benchmarkは変更しない。
既存の入力、snap、session lifecycle、dense stateをworkspace testsで引き続き検証する。
GPU benchmarkは再実行しない。実GPUが必要な既存3件はdefaultでignored。

実行結果: 上記4コマンドは全て成功。workspace testsは54件成功、3件ignored。
新規テストはcore 8件 + game 6件。Cargo.lockの開始時/終了時SHA256は
`D52C7FBFE817D6178A851C4BA09A6D66AD48EEF105F476CFA6E1B08CD23B4458` で一致した。
検証には既存targetのビルドキャッシュを使用し、source変更は専用worktreeに限定した。

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
- image hash、compressed image bytesの保持、新hostから途中参加者への画像転送。
  今回は参加済みclientが同じ画像を既に持つ前提。
- reconnect timeout、再試行、ownerがrecovery途中で再び消失した場合の交渉再開、
  重複callbackの吸収、欠落eventの再送/resync。
- runtime wiring時はgesture、pending selection/readback、overlay、queued command/event、
  GameData progress/completionを同期してから入力を再開する。
  PieceDataStoreのpresentation stateは既にresetするが、これらの別resourceのcleanupは
  次フェーズのbackend adapterが担当する。
- lost hostの最新checkpointをpeerが十分な頻度で保持する仕組みとtrust policy。
- cross-platform identity、latency/scoring、persistent saveはこのフェーズの範囲外。
