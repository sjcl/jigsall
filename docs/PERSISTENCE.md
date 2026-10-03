# ローカル進捗保存

2026-10-03。Main Menu の **Load Game** から保存一覧を開き、Pause Menu / 完成後の Puzzle Menu / 完成カードの **Save Game** からタイトルを入力して保存します。ホストのプレイ中にはオートセーブも行います。Save As・Steam Cloud は未実装です。

## オートセーブ

Settings の一般タブで有効・無効と間隔（1–60分）を変更できます。初期値は有効・5分です。変更は即時反映し、`<OS user local application data>/puzzella/settings.json` の `autosave` セクションに保存します。`interval_minutes: null` は無効、正の整数は分単位の間隔です。0や不正なJSONはエラーを表示して初期値を使います。共通のファイル形式と保存処理は [SETTINGS.md](SETTINGS.md) を参照してください。

`game/src/persistence/autosave.rs` は `GameSubState::Playing` かつ `LocalPlayerId == SessionHostId` の間だけ実時間を加算します。ポーズ・初期化・完成後・メニューでは加算しません。間隔変更・無効化・ホストでなくなった場合・セッション終了時にはタイマーをリセットします。間隔が来ても別の保存処理やタイトル入力中は待機し、空いたフレームで1回保存します。現在のローカルゲームはローカルプレイヤーがホストです。network runtime の接続・移行処理は `SessionHostId` を現在のauthorityに同期する必要があります。

初回は手動保存と別のIDを作成し、同じセッション内の次回以降は `current_autosave` のID・revisionを使って更新します。手動保存や手動保存からのロード元を上書きしません。オートセーブからロードした場合はそのIDをオートセーブ用に継続使用し、手動保存は別のIDを作成します。手動保存から新たに再開したセッションでは新しいオートセーブ枠を作ります。タイトルは既存の保存タイトルを引き継ぎ、未保存なら `Puzzle` とします。

`SaveMetadata.is_autosave` をheaderに格納し、Load Gameでは保存日時の横に `オートセーブ` / `Autosave` を表示します。ゲーム画面上部のHUDはcapture待ちからworker完了までスピナーと `オートセーブ中…` を表示します。失敗した場合はHUDにエラーを表示し、hoverで詳細を確認できます。revision競合や保存失敗では元データとID・revisionを保持し、次の間隔まで再試行しません。保存の開始要求はUI描画前、checkpoint captureはそのフレームの命令適用後に行います。

## 責務とデータフロー

```text
PieceDataStore + PuzzleDefinition + ImageHash
                ↓ 明示的 capture（committed canonical state のみ）
        PuzzleCheckpoint
          ├── GameSnapshot（session / cursor、schema 4）
          └── PuzzleSave（SaveMetadata）
                  ↓ SaveCodec / PuzImage
                 bytes
                  ↓ SaveRepository<S: SaveStorage>
                  ↓ logical StorageKey
              FilesystemStorage
```

`game/src/checkpoint.rs` が capture、validation、DSU 再構築、transactional install を所有します。`GameSnapshot` は borrowed view で同じ処理を使い、serialized fields・field order・16-byte piece layoutを維持し、schema versionは4です。位置・Z・placed・右/下の接続・flags bit 9–10のrotationを保存し、root IDs、GPU 接続 cache、selection、hover、hold、drag delta、box selection を保存しません。接続 component のrotation / rigid transform / placed 一貫性、境界外接続、rotation == 0かつ正確な placed 座標、非有限座標、Z、flags、個数を共通で検証します。

通常 Save と multiplayer snapshot は、active local / remote drag の有無に関係なく、その時点までに確定済みの canonical state を保存します。capture は `states.position` を読み、表示用の `drag.delta` を加算せず、現在の Drag を cancel / Release しません。restore では hold / holder identity / drag membership / transient delta を破棄します。`RotateDrag` で既に commit / rebase 済みの位置と回転、および Grab で更新済みの Z は保存し、rebase 後の未確定移動だけを破棄します。Grab 開始時の state に巻き戻す履歴は持ちません。migration / recovery も Drag の終了を待ちません。

`SaveRepository` は create / update / list / load / delete / image import を提供します。ランダム 128-bit `SaveId` の collision を確認し、更新は `update(id, expected_revision, title, checkpoint, original_bytes)` に session が読み込んだ revision を渡し、現在の header と一致した場合だけ同じ ID / created_at を保持して revision を増やします。`PersistenceService` は `current_save` の ID と revision を request に固定して worker に渡します。不一致は `SaveError::Conflict { id, expected_revision, actual_revision }` として、画像 import・encode・publish より前に拒否します。失敗した session の gameplay state と current_save は維持し、自動で最新 revision に付け替えて再試行しません。ロードした session の通常 Save はロード元 ID を更新します。タイトル変更にも同じ規則を適用します。タイトルは trim 後 1–80 Unicode scalar values、control / 改行禁止で、重複可能です。タイトルをファイル名に使いません。

## バイナリ形式

全整数・f32 bits は little endian。Rust の memory layout を書き出しません。未知 format version はそれぞれ拒否します。`GENERATOR_VERSION` は形状の互換性であり、save/container version や multiplayer schema と独立です。generator migration は未実装で、対応外 generator は専用エラーになります。将来の migration は codec での definition 読み取りと共通 validation の間に追加できます。

### `.puzsave` version 2

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
| 12 | SHA-256 of all preceding header bytes | 32 bytes |
| 13 | row-major piece states | 各 16 bytes |
| 14 | SHA-256 of all preceding file bytes | 32 bytes |

piece state は x f32 bits、y f32 bits、z_order u32、flags u32。flags は placed=1、connected right=2、connected down=4、bit 9–10はrotationです。他の bits は拒否します。header 長は `173 + title UTF-8 bytes`、ファイル長は `205 + title UTF-8 bytes + 16 × N` です。100万ピースで state は 16,000,000 bytes。header は最大493 bytesです。

`placed_count` は一覧用の非 authority cache です。encode 時に state から計算し、完全 decode 時に state から再計算して cache と一致を確認します。progress / completion / GameData は従来どおり install 時に state から再構築します。

一覧と更新元 metadata の取得は `read_range(key, 0, 493)` と `len(key)` だけを使います。header checksum、宣言された長さと実ファイル長、metadata、generator、definition、count と cache の範囲を検証します。piece state の読み込み・確保・DSU 再構築は行いません。50個の100万ピース save でも prefix の転送は最大24,650 bytesです（backend のプロトコル overhead は含みません）。画像は存在確認だけを行います。

Load Game の各カードには元画像のサムネイルを表示します。一覧の header から ImageHash を取得し、画像枠がスクロールの表示領域に入ったときだけ worker に読み込みを要求します。worker は該当 `.puzimg` を検証・decode し、縦横比を保って最大224×224 pixelsに縮小したRGBAだけをUIへ返します。読み込み中は画像枠にスピナーと `Loading image...` を表示します。要求は同時に1件だけで、画面外の未読画像をqueueへ積みません。読み込み開始後に画面外へ出た画像の処理は完了させます。ImageHash が同じ保存はtextureを共有し、最大64件のLRU cacheを保持します。Refresh・画面を閉じる・session変更でcacheを破棄し、古いreplyを採用しません。サムネイルの失敗は `Image unavailable` とhoverの詳細に表示し、保存一覧やResumeボタンの状態を変更しません。保存一覧とパズル本体のロードにもスピナーと専用の読み込み表示があります。

完全 load はファイル全体の checksum、header、exact state length を検証してから state 領域を確保し、共通 checkpoint validation を行います。body のみの破損は一覧では検出せず、load の失敗をその entry に表示します。truncation・過大 length・trailing bytes・checksum 不一致・invalid state はエラーです。最大1000×1000 piecesです。

オートセーブフラグの追加によりversion 2に変更しました。version 1と以前の試作layoutの読み込み互換・migrationは提供しません。一覧にはすべての対応saveの進捗を表示します。

### `.puzimg` version 1

`PUZIMG\0\0`（8 bytes）、PUZIMG_FORMAT_VERSION（u16）、payload length（u64）、ImageHash（32 bytes）、original encoded payload の順です。ImageHash は **SHA-256(original encoded image bytes)**。PNG/JPEG 等を再エンコードせず、その bytes を bit-identical に格納します。元 path / directory / filename は保存しません。encoded payload の上限は 512 MiB です。

読み取り時に magic/version、exact length、payload の SHA-256、header hash、logical storage key hash を照合します。検証済み payload だけを共通 `decode_image_bytes()` に渡し、中間 PNG/JPEG ファイルへ展開しません。

## 保存先と atomic write

```text
<OS user local application data>/puzzella/
  saves/<32 lowercase hex SaveId>.puzsave
  images/<64 lowercase hex ImageHash>.puzimg
```

Windows は `%LOCALAPPDATA%/puzzella`、macOS はユーザー Application Support 以下、Linux は XDG data directory 以下です。production は working directory に依存しません。`FilesystemStorage::new(root)` で test の temporary directory を注入できます。

storage API は `StorageKey::Save(SaveId)` / `StorageKey::Image(ImageHash)` と namespace を使います。`read_range` は指定範囲だけを読み、EOF では短い buffer を返します。backend は全 blob を取得して slice する実装を避け、`len` は blob size の metadata を取得します。任意 path、`PathBuf`、rename は上位 API にありません。固定 hex key だけから filename を作るため、タイトルによる path traversal はできません。storage-specific failure は表示可能な `StorageError` に変換します。`write(key, Vec<u8>)` は encoded allocation の所有権を渡します。repository → StorageProxy → StorageOperation::Write → owner backend の間で blob の clone は行いません。FilesystemStorage の atomic write 手順は同じです。非同期 write の executor は API が必要とする期間、受け取った buffer を保持してください。

書き込みは同じ directory の `.puzzella-*.tmp` に write → flush → sync_all → atomic overwrite。`tempfile::persist` による Windows MoveFileExW / Unix rename を使い、旧 target の delete は行いません。Unix では directory も sync します。rename/replace の失敗で旧 file を失わず、temp は一覧に入りません。電源断時の durability は OS/filesystem の保証に依存します。

プロセスの異常終了で残った一時ファイルは、起動時に persistence worker 上で1回掃除します。`saves` / `images` の直下にある `.puzzella-*.tmp` の通常ファイルだけを対象とし、最終更新から24時間以上経過したものを削除します。最近のファイル・未来の更新時刻を持つファイル・directory・symlink・保存データは対象外です。掃除は main thread をブロックせず、保存 request の処理前に完了します。directory が存在しない場合は何も作成せず、読み取り・削除の失敗は警告ログに残して他のファイルの掃除と保存処理を続けます。

新規画像の import で元 bytes の hash と既存画像 container の完全性を検証し、その後に save を publish します。import 済み画像を使う通常 Save は存在確認だけを行い、`.puzimg` の再読込・再 hash は行いません。実際の load では画像全体を検証するため、import 後の外部破損はそこで検出します。同じ ImageHash は複数 save で共有します。途中 failure は高々 orphan image を残します。save の削除では共有画像を削除しません。orphan GC は未実装です。

## Worker と restore lifecycle

画像選択 worker は元 encoded bytes と hash を確定し、同じ bytes から decode します。受信後 background repository worker が `.puzimg` を import し、成功後に encoded bytes を RAM から解放します。import failure 時は bytes を保持して後の Save で再試行できます。Save ボタンは元ファイルを読み直しません。RGBA は既存の render-only Image 方針で GPU upload 後に CPU に保持しません。

filesystem の list / read / write、codec / checksum / decode / restore 準備は crossbeam channel の worker 上で実行します。`SaveStorage` 自体には Send / Sync 制約を置きません。`PersistenceService::new(storage)` で backend を worker に移す場合だけ Send を要求します。`with_storage_requests()` は worker の `StorageProxy` と owner thread が poll する `StorageRequests` を接続し、backend handle を移動せず同じ repository 処理を使います。owner は request を同期処理するか、operation を非同期 API に dispatch し、callback から `StorageReply::complete` で返信します。待機するのは repository worker だけで、owner は event / callback loop を継続できます。executor / reply の切断は storage error として伝わります。手動保存または間隔に達したオートセーブの capture 時だけ、main world がその frame の Release 命令適用後に O(N) 処理を行います。通常 idle / pointer / camera / selection / render frame に追加する処理は channel の非 blocking poll 等で、piece 数比例の scan や新規 per-piece Entity / runtime Vec はありません。

Load は worker で `.puzsave` → `.puzimg` 検証 → 共通 decoder → checkpoint install を行い、準備済み store を main world に渡します。InGame への入場では pending restore を採用し、random placement worker を開始せず、既存 epoch / initial GPU upload を使います。Initializing 中は completion の通常イベント処理からの早期遷移も抑制し、RenderReady が対応 epoch を示した後だけ Playing / GameComplete に遷移します。ロードで hold / selection / drag、メニューへの退出で gesture / selection requests / overlay も reset します。generation token により前 session の遅延 I/O reply を破棄します。

header 破損は一覧の該当 entry だけエラーとし、body / image 破損は実際の load 時に entry のエラーに反映します。他の save はロードできます。missing image、unsupported save format、unsupported generator、corrupt save/image、storage/decode error を区別して表示します。

## 将来の Steam backend

Steam Cloud / Steam Remote Storage、Steamworks crate、Steam feature flag は未実装です。[steamworks 0.13.1 の RemoteStorage](https://docs.rs/steamworks/latest/steamworks/struct.RemoteStorage.html) は !Send / !Sync なので、handle を `PersistenceService::new` で worker に移す設計にはできません。Steam integration 側は `with_storage_requests()` の inbox を Steam 所有 thread に置き、handle をその thread に保持します。Bevy に置く場合も backend は NonSend resource として扱えます。`StorageRequest::execute(&backend)` は同期処理の便宜 API です。

[Valve の Remote Storage API](https://partner.steamgames.com/doc/api/ISteamRemoteStorage) は同期 FileRead / FileWrite が SteamAPI を block すると説明し、非同期版を推奨しています。Steam executor は operation を FileReadAsync / FileWriteAsync 等へ dispatch し、callback の成功・失敗で返信する必要があります。ReadRange は FileReadAsync の offset / length を使う経路を想定します。Rust crate で利用できる API と callback integration は実装時に確認してください。今回追加したのは request/reply の接続境界です。

`PuzzleCheckpoint`、SaveRepository、SaveCodec、PuzImage format、gameplay restore、保存メニューは共通で使います。bytes に local / steam といった backend identity は入れません。期待 revision の照合は「backend から読み取れた save が、session の読み込んだものから更新されている」場合を検出します。header 読み取りと write は atomic な compare-and-swap ではなく、同じ revision から分岐した端末や照合後の同時更新までは保証しません。[Steam Cloud](https://partner.steamgames.com/doc/features/cloud) は session 前後に同期するため、将来の integration では Steam の同期競合と競合解決も扱う必要があります。

remote の容量、atomic publish / conditional write、conflict resolution / retry、callback cancellation / shutdown は backend / executor で設計します。[Steam Cloud の現在の file size 制限](https://partner.steamgames.com/doc/features/cloud) には FileWrite / FileWriteStreamWriteChunk の1回100MB上限と、256MBでの非最適な endpoint 選択の可能性が記載されています。local container の512 MiB上限をそのまま remote API に適用せず、chunk / stream 経路や backend ごとの上限を実装時に確認してください。
