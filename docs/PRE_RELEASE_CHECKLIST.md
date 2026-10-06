# GitHub pre-release checklist

Steam 公開前の外部テスター配布用です。各 release の tag / commit、実施者、日付、
OS / GPU / driver / wgpu backend と結果を記録してください。未実施は未実施と記載します。
crash、data loss、接続不能、入力不能、著しい描画破綻は公開を止める blocker です。

## 配布前の準備

- [ ] master の最新状態を確認し、対象 commit が master の履歴に含まれる。
- [ ] workspace と Cargo.lock の全 Jigsall package が同じ version。
  最初は `0.1.0-alpha.1`、tag は `v0.1.0-alpha.1`。次の Alpha は `alpha.2` など。
  アプリの version と下記の形式番号は独立して管理する。
- [ ] GitHub repository Variables の `JIGSALL_RENDEZVOUS_WSS_URL` を運用する
  `wss://…/v1/ws` に設定する。既存の実機検証先は
  `wss://rendezvous.jigsall.sjcl.me/v1/ws`（[記録](RENDEZVOUS_V1.md)）だが、
  今回の公開先として採用するかを運用者が確認する。
- [ ] `JIGSALL_ICE_STUN_SERVERS` は公開 STUN の comma-separated `stun:host:port`
  （空も可）。`JIGSALL_ICE_ALLOW_PUBLIC_CANDIDATES` は `true` / `false`
  （release workflow の既定は `true`）を確認する。
- [ ] Actions はこの3つだけを Git ignored の `internet-defaults.env` に書く。
  ローカルでは `internet-defaults.env.example` をコピーする。runtime 環境変数が
  build-time default より優先される。endpoint 未設定・placeholder・不正値は
  release build を失敗させる。秘密値のキーを追加しない。
- [ ] TURN provider の長期 credential / secret は rendezvous server 側の
  Secrets / deployment 設定で管理する。client binary / GitHub Variables /
  `internet-defaults.env` へ入れない。client は runtime に短期 TURN credential
  を受け取る。配布 ZIP にローカル設定ファイルを入れない。
- [ ] プロジェクト本体の LICENSE を所有者が確認する。現時点でルートに LICENSE
  はなく、この作業では本体の license を決定していない。

## 互換性の境界

| 対象 | 現在 | 判定箇所 / 保護 |
| --- | --- | --- |
| `.puz` 保存 | `SAVE_FORMAT_VERSION = 1` | `persistence/codec.rs`、magic・長さ・header/body checksum・定義/状態検証 |
| `.puzimg` 原画像 | `PUZIMG_FORMAT_VERSION = 1` | 同 codec、長さ・SHA256・storage key 検証 |
| generator / game definition | `GENERATOR_VERSION = 1` | core `PuzzleDefinition::validate`、保存と snapshot の読込でも要求 |
| multiplayer wire / Postcard payload | `WIRE_VERSION = 1` | `network/wire.rs`、deserialize 前の header gate。secure channel の鍵導出 / AAD もこの番号に結合 |
| multiplayer snapshot | `SNAPSHOT_SCHEMA_VERSION = 1` | `multiplayer/snapshot.rs`、install 前の検証 |
| join baseline | `JOIN_BASELINE_SCHEMA_VERSION = 1` | `multiplayer/baseline.rs`、snapshot と drag の検証 |
| rendezvous signaling | JSON `v = 1` | `gns/rendezvous/protocol.rs`、typed parse で異なる version を拒否 |
| settings JSON | `format_version = 1` | `settings_file.rs`、未知・不正 version は読込と書込を拒否。未指定の開発版は同じ v1 layout として扱う |

これらは対応する番号との完全一致だけを受け付ける。Alpha 間の完全な後方互換性や
migration は提供しない。公開済みの番号の byte layout、enum 順序、flags、seed/hash、
形状・配置の意味を後から変えない。breaking change では影響する番号を増やし、
保存の header / body や wire の nested payload の変更は外側の番号も更新する。
generator の描画・picking 共通形状を変える場合も定義の再構成に影響するか確認する。

保存の restore は検証・準備後に適用される。repository 更新は既存 header の
version / revision を確認し、filesystem は同一 directory の temporary file を
sync して置換する。画像 publish が失敗した場合は save を publish しない。
load 失敗を保存成功として扱わない。settings は write worker でも version を再確認する。

## Single-player

- [ ] 新規 puzzle、100 / 1,000 / 10,000 pieces。
- [ ] rotation off / on、左右回転、drag、connected snapping、board snap。
- [ ] Ctrl selection、rectangle selection、相対位置を保つ multi-drag。
- [ ] camera pan / zoom、drag release の最終座標と snap。
- [ ] save、restart、load、autosave、puzzle completion。
- [ ] 完成済み save を再開して終了、未保存状態の終了確認と保存失敗後の再試行。

## Image

- [ ] PNG、JPEG、WebP、alpha PNG（透明穴・半透明）。
- [ ] portrait / landscape、画像端・細長い画像。
- [ ] 画像の alpha と main / point / rectangle picking の一致。

## Large-scale smoke test

- [ ] 100k pieces の生成・基本操作・保存 / load。
- [ ] 1M pieces の生成・全体表示・zoom・pan・基本 drag / selection。
- [ ] crash / corruption がないこと。快適性や 60 fps の保証には使わない。

## Multiplayer

- [ ] Direct IP host / join（同一 LAN と外部接続）。
- [ ] Room Code host / join（別 network / NAT）。
- [ ] password authentication：正しい / 間違った password、再試行。
- [ ] join-in-progress、image transfer、state sync、回転と connected drag。
- [ ] client disconnect、host disconnect、disconnected client の last state save / load。
- [ ] rendezvous signaling 切断後に既存 P2P connection で操作・状態同期が続く。
- [ ] direct 接続できない実環境で TURN fallback を確認する。使用経路を記録する。
  短期 credential の期限 / server failure 時に既存接続を壊さない。
- [ ] Windows / Linux 間の host / client 双方向を実機で確認する。

## Persistence failure

テスト専用 data directory / 複製した save を使い、元のユーザーデータを保管する。

- [ ] invalid save、unsupported save / image / generator version。
- [ ] truncated / corrupt header、body、image、checksum mismatch。
- [ ] write failure（permission / disk full / repository lock）。
- [ ] 失敗後に既存 save / image が同じ bytes のまま、進行状態を誤適用しない。
- [ ] unsupported / corrupt settings の変更を試し、元の settings.json を上書きしない。

## Display / input

- [ ] windowed / fullscreen、DPI scaling 100 / 150 / 200%。
- [ ] Alt+F4 / window close と save / discard / cancel。
- [ ] camera drag 中に cursor が window 外へ出る、focus loss、pause、復帰。
- [ ] raw camera pan の速度が DPI で変わらず、cursor lock が解除される。
- [ ] Low / Medium / High 品質、bevel の凸 tab / 凹 slot / connected seam、
  pseudo-3D の shadow / side、camera zoom / rotation、透明画像を目視確認する。

## 自動検証と archive

- [ ] `cargo fmt --all --check`、default / all-features Clippy。
- [ ] workspace tests、all-features build / tests / doctests。
- [ ] Windows rendezvous / GNS build と直列 localhost tests。
- [ ] Linux CI 相当の build / tests、release build の両 OS 成功。
- [ ] `.github/scripts/test_release.py` と actionlint（公式 runner label を確認）。
- [ ] `python .github/scripts/install_cargo_about.py` で SHA256 固定の cargo-about
  0.8.4 を取得し、`release.py licenses` を再生成できる。未知 license は失敗させる。
  [cargo-about の仕様](https://embarkstudios.github.io/cargo-about/)を参照。
- [ ] Rust dependencies（Bevy / egui と既定フォントを含む）、GNS wrappers、
  Valve GNS 内部コード、M PLUS 1p、OpenSSL / Protobuf と native 依存の通知を確認。
  Windows はそのビルドの vcpkg installed copyright / version、Linux は distro
  package の copyright / version を収集する。
- [ ] `jigsall-windows-x86_64.zip` / `jigsall-linux-x86_64.tar.gz` を展開する。
  `jigsall/` 内に executable、README、PLAYING、checklist、release notes template、
  `BUILD_INFO.txt`、font license、`THIRD_PARTY_LICENSES.txt`、
  `THIRD_PARTY_NOTICES.txt`、`SHA256SUMS` がある。
- [ ] archive 内の SHA256SUMS が各 payload と一致し、Release 添付 SHA256SUMS が
  両 archive と一致する。Linux executable の実行 permission がある。
- [ ] clean な Windows / Linux で executable を起動できる（native runtime の不足も確認）。
  Windows は [Visual C++ v14 Redistributable x64](https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist)
  を前提とする。PE imports の MSVCP140 / VCRUNTIME140 / VCRUNTIME140_1 は
  Microsoft の runtime installer で提供し、配布 ZIP に DLL を追加しない。
  Linux は Ubuntu 24.04 の build を対象 OS で確認する。
  archive metadata は固定するが、compiler / native dependency / OS が異なる
  binary の bit 再現性は主張しない。
- [ ] title UI、BUILD_INFO、診断ログの version / commit、GPU / backend を確認する。
- [ ] master ancestry gate と draft-only release を維持する。Alpha tag は prerelease。
  公開済み release を workflow で更新しない。
- [ ] [release notes template](RELEASE_NOTES_TEMPLATE.md) の known issues と検証結果を埋め、
  GitHub Draft に貼り付ける。人間が結果を確認してから publish する。
