# ローカル進捗保存

2026-10-02。Main Menu の **Load Game** から保存一覧を開き、Pause Menu / 完成後の Puzzle Menu / 完成カードの **Save Game** からタイトルを入力して保存します。自動保存・Save As・Steam Cloud は未実装です。

## 責務とデータフロー

```text
PieceDataStore + PuzzleDefinition + ImageHash
                ↓ 明示的 capture（active local drag は拒否）
        PuzzleCheckpoint
          ├── GameSnapshot（session / cursor、既存 schema 3）
          └── PuzzleSave（SaveMetadata）
                  ↓ SaveCodec / PuzImage
                 bytes
                  ↓ SaveRepository<S: SaveStorage>
                  ↓ logical StorageKey
              FilesystemStorage
```

`game/src/checkpoint.rs` が capture、validation、DSU 再構築、transactional install を所有します。`GameSnapshot` は borrowed view で同じ処理を使い、serialized fields・field order・schema version 3 を維持します。位置・Z・placed・右/下の接続のみを保存し、root IDs、GPU 接続 cache、selection、hover、hold、drag delta、box selection を保存しません。接続 component の offset / placed 一貫性、境界外接続、正確な placed 座標、非有限座標、Z、flags、個数を共通で検証します。

`SaveRepository` は create / update / list / load / delete / image import を提供します。ランダム 128-bit `SaveId` の collision を確認し、更新は同じ ID / created_at を保持して revision を増やします。ロードした session の通常 Save はロード元 ID を更新します。タイトル変更にも同じ規則を適用します。タイトルは trim 後 1–80 Unicode scalar values、control / 改行禁止で、重複可能です。タイトルをファイル名に使いません。

## バイナリ形式

全整数・f32 bits は little endian。Rust の memory layout を書き出しません。未知 format version はそれぞれ拒否します。`GENERATOR_VERSION` は形状の互換性であり、save/container version や multiplayer schema と独立です。generator migration は未実装で、対応外 generator は専用エラーになります。将来の migration は codec での definition 読み取りと共通 validation の間に追加できます。

### `.puzsave` version 1

| 順序 | フィールド | 幅 |
| --- | --- | --- |
| 1 | magic `PUZSAVE\0` | 8 bytes |
| 2 | SAVE_FORMAT_VERSION | u16 |
| 3 | SaveId | u128 |
| 4 | revision / created_at / updated_at | 各 u64（時刻は UTC Unix seconds） |
| 5 | title byte length / UTF-8 title | u32 / 可変長 |
| 6 | ImageHash | 32 bytes |
| 7 | generator_version / seed | u16 / u64 |
| 8 | grid width / height / image width / height | 各 u32 |
| 9 | snap_distance | f32 bits |
| 10 | next_z_order / piece count | 各 u32 |
| 11 | row-major piece states | 各 16 bytes |
| 12 | SHA-256 of all preceding bytes | 32 bytes |

piece state は x f32 bits、y f32 bits、z_order u32、flags u32。flags は placed=1、connected right=2、connected down=4。他の bits は拒否します。ファイル長は `156 + title UTF-8 bytes + 16 × N` です。100万ピースで state は 16,000,000 bytes。placed_count / progress / completion は authority として重複保存せず、install 時に再計算して `GameData` に同期します。

magic/version、総 length 上限、checksum、UTF-8/title、metadata、generator、definition、piece count、残り length を検証してから state 領域を確保します。truncation・過大 length・trailing bytes・checksum 不一致・invalid state はエラーです。最大 1000×1000 pieces に制限します。

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

storage API は `StorageKey::Save(SaveId)` / `StorageKey::Image(ImageHash)` と namespace を使います。任意 path、`PathBuf`、rename は上位 API にありません。固定 hex key だけから filename を作るため、タイトルによる path traversal はできません。storage-specific failure は表示可能な `StorageError` に変換します。

書き込みは同じ directory の `.puzzella-*.tmp` に write → flush → sync_all → atomic overwrite。`tempfile::persist` による Windows MoveFileExW / Unix rename を使い、旧 target の delete は行いません。Unix では directory も sync します。rename/replace の失敗で旧 file を失わず、temp は一覧に入りません。電源断時の durability は OS/filesystem の保証に依存します。

画像を先に保存・検証し、その後に save を publish します。同じ ImageHash は複数 save で共有し、存在する画像 container も検証します。途中 failure は高々 orphan image を残します。save の削除では共有画像を削除しません。orphan GC は未実装です。

## Worker と restore lifecycle

画像選択 worker は元 encoded bytes と hash を確定し、同じ bytes から decode します。受信後 background repository worker が `.puzimg` を import し、成功後に encoded bytes を RAM から解放します。import failure 時は bytes を保持して後の Save で再試行できます。Save ボタンは元ファイルを読み直しません。RGBA は既存の render-only Image 方針で GPU upload 後に CPU に保持しません。

list / read / write / checksum / decode / restore 準備は crossbeam channel の worker 上で実行します。明示的な Save の capture だけ、UI の request を受けた main world がその frame の Release 命令適用後に O(N) 処理を行います。通常 idle / pointer / camera / selection / render frame に追加する処理は channel の非 blocking poll 等で、piece 数比例の scan や新規 per-piece Entity / runtime Vec はありません。

Load は worker で `.puzsave` → `.puzimg` 検証 → 共通 decoder → checkpoint install を行い、準備済み store を main world に渡します。InGame への入場では pending restore を採用し、random placement worker を開始せず、既存 epoch / initial GPU upload を使います。Initializing 中は completion の通常イベント処理からの早期遷移も抑制し、RenderReady が対応 epoch を示した後だけ Playing / GameComplete に遷移します。ロードで hold / selection / drag、メニューへの退出で gesture / selection requests / overlay も reset します。generation token により前 session の遅延 I/O reply を破棄します。

破損 save は一覧の該当 entry だけエラーとし、他の save はロードできます。missing image、unsupported save format、unsupported generator、corrupt save/image、storage/decode error を区別して表示します。

## 将来の Steam backend

Steam Cloud / Steam Remote Storage、Steamworks crate、Steam feature flag は今回追加していません。将来は `SaveStorage` implementation とその構築・認証 adapter を追加し、repository / `PersistenceService::new(storage)` に注入することが境界です。`PuzzleCheckpoint`、SaveCodec、PuzImage format、gameplay restore、保存メニューを変更しないことを設計目標とします。bytes に local / steam といった backend identity は入れません。remote の容量・atomic publish・conflict resolution / retry は backend で設計する必要があり、revision は今回ローカル更新の counter として提供します。
