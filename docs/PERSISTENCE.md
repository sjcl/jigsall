# ローカル進捗保存

2026-10-04。Main Menu の **Load Game** から保存一覧を開き、Pause Menu / 完成後の Puzzle Menu / 完成カードの **Save Game** からタイトルを入力して保存します。ホストのプレイ中にはオートセーブも行います。Save As・Steam Cloud は未実装です。

## オートセーブ

Settings の一般タブで有効・無効、間隔（1–60分）、ゲームごとの保存件数上限（1件以上）を変更できます。初期値は有効・5分・1件です。変更は即時反映し、`<OS user local application data>/puzzella/settings.json` の `autosave` セクションに保存します。`interval_minutes: null` は無効、正の整数は分単位の間隔です。`max_saves_per_game` はゲームごとに保持するオートセーブ件数で、省略時は1です。0や不正なJSONはエラーを表示して初期値を使います。上限を減らした場合は、そのゲームの次のオートセーブ成功後に超過分を削除します。共通のファイル形式と保存処理は [SETTINGS.md](SETTINGS.md) を参照してください。

`game/src/persistence/autosave.rs` は `GameSubState::Playing` かつ `LocalPlayerId == SessionHostId` の間だけ実時間を加算します。ポーズ・初期化・完成後・メニューでは加算しません。間隔変更・無効化・ホストでなくなった場合・セッション終了時にはタイマーをリセットします。間隔が来ても別の保存処理やタイトル入力中は待機し、空いたフレームで1回保存します。現在のローカルゲームはローカルプレイヤーがホストです。network runtime の接続・移行処理は `SessionHostId` を現在のauthorityに同期する必要があります。

新規ゲーム開始時に `uuid::Uuid::new_v4()` でUUID v4の `GameId` を生成し、128-bit整数として保持します。クライアント間で採番を調整せず、画像や生成条件が同じでも新規ゲームには別のIDを使います。`SaveMetadata.game_id` に保存し、手動保存・オートセーブのロードでは同じIDを引き継ぎます。`SaveId` は各保存ファイルを識別する別のIDです。

オートセーブは毎回新しい `SaveId` にcheckpointを保存し、成功後に同じ `GameId` の古いオートセーブだけを削除して最新の指定件数を保持します。保存日時が同じ秒の場合は、ゲームのオートセーブ間で増加するrevisionを使って順序を判別します。時刻の巻き戻りでも直前の保存日時を下回りません。手動保存や他のゲーム、headerが不正・未対応の保存は削除しません。共有画像も保持します。タイトルは既存の保存タイトルを引き継ぎ、未保存なら `Puzzle` とします。一覧・header読み込み・保存・削除はworker上で行い、履歴のpiece stateは読み込みません。

`SaveMetadata.is_autosave` をheaderに格納し、Load Gameでは保存日時の横に `オートセーブ` / `Autosave` を表示します。ゲーム画面上部のHUDはcapture待ちからworker完了までスピナーと `オートセーブ中…` を表示します。失敗した場合はHUDにエラーを表示し、hoverで詳細を確認できます。前回のオートセーブのrevisionが競合した場合や保存失敗では元データとID・revisionを保持し、次の間隔まで再試行しません。削除だけ失敗した場合は新しい保存のID・revisionを採用し、超過分を残してHUDにエラーを表示します。次のオートセーブ成功後に削除を再試行します。保存の開始要求はUI描画前、checkpoint captureはそのフレームの命令適用後に行います。

## 責務とデータフロー

```text
PieceDataStore + PuzzleDefinition + ImageHash
                ↓ 明示的 capture（committed canonical state のみ）
        PuzzleCheckpoint
          ├── GameSnapshot（session / cursor、schema 5）
          └── PuzzleSave（SaveMetadata）
                  ↓ SaveCodec / PuzImage
                 bytes
                  ↓ SaveRepository<S: SaveStorage>
                  ↓ logical StorageKey
              FilesystemStorage
```

`game/src/checkpoint.rs` が capture、validation、DSU 再構築、transactional install を所有します。`GameSnapshot` は borrowed view で同じ処理を使い、definitionにrotation_enabledを含め、16-byte piece layoutを維持し、schema versionは5です。位置・Z・placed・右/下の接続・flags bit 9–10のrotationを保存し、root IDs、GPU 接続 cache、selection、hover、hold、drag delta、box selection を保存しません。接続 component のrotation / rigid transform / placed 一貫性、境界外接続、rotation == 0かつ正確な placed 座標、非有限座標、Z、flags、個数を共通で検証します。

通常 Save と multiplayer snapshot は、active local / remote drag の有無に関係なく、その時点までに確定済みの canonical state を保存します。capture は `states.position` を読み、表示用の `drag.delta` を加算せず、現在の Drag を cancel / Release しません。restore では hold / holder identity / drag membership / transient delta を破棄します。`RotateDrag` で既に commit / rebase 済みの位置と回転、および Grab で更新済みの Z は保存し、rebase 後の未確定移動だけを破棄します。Grab 開始時の state に巻き戻す履歴は持ちません。migration / recovery も Drag の終了を待ちません。

`SaveRepository` は create / update / list / load / delete / image import を提供します。ランダム 128-bit `SaveId` の collision を確認し、更新は `update(id, expected_revision, title, checkpoint, original_bytes)` に session が読み込んだ revision を渡し、現在の header と一致した場合だけ同じ ID / created_at を保持して revision を増やします。`PersistenceService` は `current_save` の ID と revision を request に固定して worker に渡します。不一致は `SaveError::Conflict { id, expected_revision, actual_revision }` として、画像 import・encode・publish より前に拒否します。失敗した session の gameplay state と current_save は維持し、自動で最新 revision に付け替えて再試行しません。ロードした session の通常 Save はロード元 ID を更新します。タイトル変更にも同じ規則を適用します。タイトルは trim 後 1–80 Unicode scalar values、control / 改行禁止で、重複可能です。タイトルをファイル名に使いません。

## バイナリ形式

全整数・f32 bits は little endian。Rust の memory layout を書き出しません。未知 format version はそれぞれ拒否します。`GENERATOR_VERSION` は形状の互換性であり、save/container version や multiplayer schema と独立です。generator migration は未実装で、対応外 generator は専用エラーになります。将来の migration は codec での definition 読み取りと共通 validation の間に追加できます。

### `.puzsave` version 4

| 順序 | フィールド | 幅 |
| --- | --- | --- |
| 1 | magic `PUZSAVE\0` / SAVE_FORMAT_VERSION | 8 bytes / u16 |
| 2 | header byte length / total file byte length | u32 / u64 |
| 3 | SaveId | u128 |
| 4 | revision / created_at / updated_at | 各 u64（時刻は UTC Unix seconds） |
| 5 | title byte length / UTF-8 title | u32 / 可変長 |
| 6 | ImageHash | 32 bytes |
| 7 | generator_version / seed | u16 / u64 |
| 8 | grid width / height / image width / height | 各 u32 |
| 9 | snap_distance | f32 bits |
| 10 | next_z_order / piece count / placed_count cache | 各 u32 |
| 11 | is_autosave（0=手動、1=自動、他の値は拒否） | u8 |
| 12 | GameId | u128 |
| 13 | rotation_enabled（0=回転なし、1=回転あり、他の値は拒否） | u8 |
| 14 | SHA-256 of all preceding header bytes | 32 bytes |
| 15 | row-major piece states | 各 16 bytes |
| 16 | SHA-256 of all preceding file bytes | 32 bytes |

piece state は x f32 bits、y f32 bits、z_order u32、flags u32。flags は placed=1、connected right=2、connected down=4、bit 9–10はrotationです。回転なしのゲームでは非ゼロrotationも拒否します。他の bits は拒否します。header 長は `190 + title UTF-8 bytes`、ファイル長は `222 + title UTF-8 bytes + 16 × N` です。100万ピースで state は 16,000,000 bytes。header は最大510 bytesです。

`placed_count` は一覧用の非 authority cache です。encode 時に state から計算し、完全 decode 時に state から再計算して cache と一致を確認します。progress / completion / GameData は従来どおり install 時に state から再構築します。

一覧と更新元 metadata の取得は `read_range(key, 0, 510)` と `len(key)` だけを使います。header checksum、宣言された長さと実ファイル長、metadata、generator、definition、count と cache の範囲を検証します。piece state の読み込み・確保・DSU 再構築は行いません。50個の100万ピース save でも prefix の転送は最大25,500 bytesです（backend のプロトコル overhead は含みません）。画像は存在確認だけを行います。

Load Game の各カードには元画像のサムネイルを表示します。一覧の header から ImageHash を取得し、画像枠がスクロールの表示領域に入ったときだけ専用 worker に読み込みを要求します。サムネイルは保存・一覧・ロードの request queue と worker を共有しません。通常の filesystem 経路では同じ保存先の独立した storage handle を使い、元画像の読み込み・SHA-256検証・フルサイズ decode・縮小をサムネイル worker 上で行います。縦横比を保って最大224×224 pixelsに縮小したRGBAだけをUIへ返します。初回とcache eviction後には元画像全体の処理が必要ですが、この処理の完了待ちをロードや保存のworkerに持ち込みません。

読み込み中は画像枠にスピナーと `Loading image...` を表示します。要求は同時に1件だけで、画面外の未読画像をqueueへ積みません。読み込み開始後に画面外へ出た画像の処理は完了させます。ImageHash が同じ保存はtextureを共有し、最大64件のLRU cacheを保持します。成功したtextureはメニューを閉じた後やsession変更後も再利用します。同じsessionでメニューを閉じた後に完了したreplyもcacheへ格納します。session変更前の未完了replyは採用せず、完了まで新しいサムネイル要求を待たせてqueueの上限を維持します。失敗したcacheはメニュー再表示・session変更時に除去して再試行できるようにし、Refreshは成功したtextureも含めてcacheを破棄します。サムネイルの失敗は `Image unavailable` とhoverの詳細に表示し、保存一覧やResumeボタンの状態を変更しません。保存一覧とパズル本体のロードにもスピナーと専用の読み込み表示があります。

完全 load はファイル全体の checksum、header、exact state length を検証してから state 領域を確保し、共通 checkpoint validation を行います。body のみの破損は一覧では検出せず、load の失敗をその entry に表示します。truncation・過大 length・trailing bytes・checksum 不一致・invalid state はエラーです。最大1000×1000 piecesです。

回転モードの追加によりversion 4に変更しました。version 1・2・3と以前の試作layoutは拒否し、読み込み互換・migrationは提供しません。一覧にはすべての対応saveの進捗を表示します。

### `.puzimg` version 1

`PUZIMG\0\0`（8 bytes）、PUZIMG_FORMAT_VERSION（u16）、payload length（u64）、ImageHash（32 bytes）、original encoded payload の順です。ImageHash は **SHA-256(original encoded image bytes)**。PNG/JPEG 等を再エンコードせず、その bytes を bit-identical に格納します。元 path / directory / filename は保存しません。encoded payload の上限は 512 MiB です。

読み取り時に magic/version、exact length、payload の SHA-256、header hash、logical storage key hash を照合します。検証済み payload だけを共通 `decode_image_bytes()` に渡し、中間 PNG/JPEG ファイルへ展開しません。workerは元画像の寸法から共通の整数演算でlogical size（最大辺16384 px）を求め、保存definitionのimage_sizeと照合します。texture寸法との照合は行わず、その端末のGPU辺上限・画像予算へ縮小したtextureをAssetsへ登録します。異なる端末の予算でも保存された座標系は変わりません。definitionの0寸法・16384超過はsave / snapshot共通validationで拒否します。以前の16384超過definitionを含む保存の自動移行は行いません。

## 保存先と atomic write

```text
<OS user local application data>/puzzella/
  saves/<32 lowercase hex SaveId>.puzsave
  images/<64 lowercase hex ImageHash>.puzimg
  locks/repository.lock
  locks/<64 lowercase hex ImageHash>.puzimg.lock
```

Windows は `%LOCALAPPDATA%/puzzella`、macOS はユーザー Application Support 以下、Linux は XDG data directory 以下です。production は working directory に依存しません。`FilesystemStorage::new(root)` で test の temporary directory を注入できます。

storage API は `StorageKey::Save(SaveId)` / `StorageKey::Image(ImageHash)` と namespace を使います。`read_range` は指定範囲だけを読み、EOF では短い buffer を返します。backend は全 blob を取得して slice する実装を避け、`len` は blob size の metadata を取得します。任意 path、`PathBuf`、rename は上位 API にありません。固定 hex key だけから filename を作るため、タイトルによる path traversal はできません。storage-specific failure は表示可能な `StorageError` に変換します。`write(key, Vec<u8>)` は encoded allocation の所有権を渡します。repository → StorageProxy → StorageOperation::Write → owner backend の間で blob の clone は行いません。FilesystemStorage の atomic write 手順は同じです。非同期 write の executor は API が必要とする期間、受け取った buffer を保持してください。

書き込みは同じ directory の `.puzzella-*.tmp` に write → flush → sync_all → atomic overwrite。`tempfile::persist` による Windows MoveFileExW / Unix rename を使い、旧 target の delete は行いません。Unix では directory も sync します。rename/replace の失敗で旧 file を失わず、temp は一覧に入りません。電源断時の durability は OS/filesystem の保証に依存します。

プロセスの異常終了で残った一時ファイルは、起動時に persistence worker 上で1回掃除します。`saves` / `images` の直下にある `.puzzella-*.tmp` の通常ファイルだけを対象とし、最終更新から24時間以上経過したものを削除します。最近のファイル・未来の更新時刻を持つファイル・directory・symlink・保存データは対象外です。掃除は main thread をブロックせず、保存 request の処理前に完了します。directory が存在しない場合は何も作成せず、読み取り・削除の失敗は警告ログに残して他のファイルの掃除と保存処理を続けます。

新規画像の import で元 bytes の hash と既存画像 container の完全性を検証し、その後に save を publish します。import 済み画像を使う通常 Save は存在確認だけを行い、`.puzimg` の再読込・再 hash は行いません。実際の load では画像全体を検証するため、import 後の外部破損はそこで検出します。同じ ImageHash は複数 save で共有します。途中 failure は高々 orphan image を残します。

save の削除に成功した後、persistence worker 上で残ったすべての save の検証済み header から ImageHash を集め、どの save にも参照されていない `.puzimg` を削除します。画像ごとの専用 lock file に対する排他ロックを待たずに取得し、使用中の共有ロックがあればその画像の掃除を見送ります。手動保存・オートセーブ・実行中の画像選択やゲームのどれかに参照があれば画像を保持し、以前の更新・保存失敗・使用終了した未保存画像の orphan も掃除します。piece state や画像 payload の読み込み・decode は行いません。残った save の header が破損・未対応・読み取り失敗の場合、参照を確定できないため画像の掃除全体を見送ります。掃除の失敗は警告ログに残し、成功済みの save 削除と一覧更新は継続します。画像の削除・ロック取得が失敗した場合は他の未参照画像の掃除を続け、残った画像は次回の save 削除時に再試行します。存在しない save の再削除でも掃除を実行します。

FilesystemStorage のプロセス間協調は Rust 標準の `File::try_lock` / `try_lock_shared` を使い、OS 固有の削除禁止フラグやプロセス同士の直接通信には依存しません。`repository.lock` の排他ロックで import、create、update、autosave の公開・ローテーション、delete の参照確認から画像掃除までを直列化します。これにより画像の公開から保護取得までの隙間、掃除の参照確認後の save 公開、同じ revision への同時更新を防ぎます。load は同じロック内で save を読み、画像の共有ロックを取得してから共通ロックを解放し、画像の読み込み・decode を続けます。`locks` の空ファイルは削除・置換しません。同じ名前で再作成すると別の filesystem object のロックに分かれるためです。ロックの有効性はファイルの存在や PID・時刻ではなく保持中の handle で決まり、最後の guard の解放やプロセス終了で OS が解放します。

`SaveStorage` の lock hook は非 blocking の試行と `StorageGuard` を返します。`StorageProxy` もこの試行だけを owner へ転送し、競合時の再試行待ちは repository worker 上で行います。ロック保持者が次の I/O を待つ間に executor を停止させません。hook の既定値は単一 client の private backend 向けで、複数 client が共有する backend は3つの hook を実装する必要があります。

repository の排他ロックと画像の共有ロックの取得待ちは、各取得につき単調増加時計で30秒を上限とします。競合が続けば `StorageError::LockTimeout` を返し、保存・ロード等の処理中表示を解除してエラーを表示します。手動保存の失敗時はタイトル入力を開いたままにし、オートセーブの失敗は専用のエラー表示に反映します。現在のゲーム状態・保存 metadata は維持し、後で再試行できます。タイムアウトしても他のインスタンスのロックを解除したり、lock file を削除したりしません。

## Worker と restore lifecycle

画像選択 worker は元 encoded bytes と hash を確定し、同じ bytes から decode します。受信後 background repository worker が画像の共有ロックを保持して `.puzimg` を import し、成功応答から `OriginalPuzzleImage` へ `ImageLease` を移してから、通常は encoded bytes を RAM から解放します。load / Save の成功応答も lease を保持します。lease は選択中・Playing・pause・完成画面で維持し、画像の置き換え・session cleanup で解放します。送信済み Save request も lease の clone を持つため、session cleanup 後の保存待ち中も保護されます。古い generation / 別画像の応答やロード失敗は guard を drop します。import / lock failure 時は bytes を保持して後の Save で再試行できます。Save ボタンは元ファイルを読み直しません。RGBA は既存の render-only Image 方針で GPU upload 後に CPU に保持しません。

マルチプレイのホスト準備では `PersistenceState.retain_image_for_host` を設定し、import / Save 成功後も encoded bytes の共有 Arc と lease の両方を保持します。ホスト用 `load_for_host` は検証済み encoded bytes と lease を worker から返します。通常ロードは引き続き encoded bytes を保持せず、Menu cleanup は保持設定と元画像を破棄します。

filesystem の list / read / write、codec / checksum / decode / restore 準備は crossbeam channel の worker 上で実行します。`SaveStorage` 自体には Send / Sync 制約を置きません。`PersistenceService::new(storage)` で backend をI/O workerに移す場合だけ Send を要求し、Clone / Sync は要求しません。この汎用経路はI/Oを直列化しますが、別々の foreground / thumbnail worker がchecksum・decode等を行うため、サムネイルのCPU処理は保存・ロードを待たせません。通常のfilesystem経路は独立したhandleを使い、サムネイルのI/Oもforeground workerから分離します。

`with_storage_requests()` は両workerの `StorageProxy` と owner thread が poll する `StorageRequests` を接続し、backend handle を移動せず同じ repository 処理を使います。owner は request を同期処理するか、operation を非同期 API に dispatch し、callback から `StorageReply::complete` で返信します。foregroundとサムネイルのI/O要求は同時に未完了になり得るため、非同期executorは1件の完了を待たずに次のoperationをdispatchできます。待機するのは要求元workerだけで、owner は event / callback loop を継続できます。executor / reply の切断は storage error として伝わります。手動保存または間隔に達したオートセーブの capture 時だけ、main world がその frame の Release 命令適用後に O(N) 処理を行います。通常 idle / pointer / camera / selection / render frame に追加する処理は channel の非 blocking poll 等で、piece 数比例の scan や新規 per-piece Entity / runtime Vec はありません。

Load は worker で `.puzsave` → `.puzimg` 検証 → 共通 decoder → checkpoint install を行い、準備済み store を main world に渡します。InGame への入場では pending restore を採用し、random placement worker を開始せず、既存 epoch / initial GPU upload を使います。Initializing 中は completion の通常イベント処理からの早期遷移も抑制し、RenderReady が対応 epoch を示した後だけ Playing / GameComplete に遷移します。ロードで hold / selection / drag、メニューへの退出で gesture / selection requests / overlay も reset します。generation token により前 session の遅延 I/O reply を破棄します。

header 破損は一覧の該当 entry だけエラーとし、body / image 破損は実際の load 時に entry のエラーに反映します。他の save はロードできます。missing image、unsupported save format、unsupported generator、corrupt save/image、storage/decode error を区別して表示します。

## 将来の Steam backend

Steam Cloud / Steam Remote Storage、Steamworks crate、Steam feature flag は未実装です。[steamworks 0.13.1 の RemoteStorage](https://docs.rs/steamworks/latest/steamworks/struct.RemoteStorage.html) は !Send / !Sync なので、handle を `PersistenceService::new` で worker に移す設計にはできません。Steam integration 側は `with_storage_requests()` の inbox を Steam 所有 thread に置き、handle をその thread に保持します。Bevy に置く場合も backend は NonSend resource として扱えます。`StorageRequest::execute(&backend)` は同期処理の便宜 API です。

[Valve の Remote Storage API](https://partner.steamgames.com/doc/api/ISteamRemoteStorage) は同期 FileRead / FileWrite が SteamAPI を block すると説明し、非同期版を推奨しています。Steam executor は operation を FileReadAsync / FileWriteAsync 等へ dispatch し、callback の成功・失敗で返信する必要があります。ReadRange は FileReadAsync の offset / length を使う経路を想定します。Rust crate で利用できる API と callback integration は実装時に確認してください。今回追加したのは request/reply の接続境界です。

`PuzzleCheckpoint`、SaveRepository、SaveCodec、PuzImage format、gameplay restore、保存メニューは共通で使います。bytes に local / steam といった backend identity は入れません。期待 revision の照合は「backend から読み取れた save が、session の読み込んだものから更新されている」場合を検出します。header 読み取りと write は atomic な compare-and-swap ではなく、同じ revision から分岐した端末や照合後の同時更新までは保証しません。[Steam Cloud](https://partner.steamgames.com/doc/features/cloud) は session 前後に同期するため、将来の integration では Steam の同期競合と競合解決も扱う必要があります。

remote の容量、atomic publish / conditional write、conflict resolution / retry、callback cancellation / shutdown は backend / executor で設計します。[Steam Cloud の現在の file size 制限](https://partner.steamgames.com/doc/features/cloud) には FileWrite / FileWriteStreamWriteChunk の1回100MB上限と、256MBでの非最適な endpoint 選択の可能性が記載されています。local container の512 MiB上限をそのまま remote API に適用せず、chunk / stream 経路や backend ごとの上限を実装時に確認してください。
