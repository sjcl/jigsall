# ネットワークの責務と失敗方針

Wire形式と互換性番号は変更していません。ゲーム状態の正本・sequence tracker・
authority cursorの所有者も維持しています。

## Runtimeの境界

- `runtime.rs`：transport poll、roleごとの処理呼び出し、画像decode、ローカルcommand、teardown。
- `runtime/lifecycle.rs`：接続イベントとbootstrap gate、live集合による一度だけの後始末。
- `runtime/host.rs`：Syncの開始・進行、Gameplayのapply→retain→publish、Presence、切断時cancel。
- `runtime/client.rs`：Syncの結果適用、replicaと予測状態のreconcile。

送信は全Readyピアへ試行してから失敗した接続を後始末します。切断の連鎖はpendingリストで
処理し、cancelの適用やPresenceのrevision更新はlive集合からの削除だけが所有します。
保持失敗はcatch-upのrestartを進めつつ通常配信を継続し、commandを再適用しません。

## Sync・描画・Direct-IPの境界

| モジュール | 責務 |
| --- | --- |
| `syncing.rs` | SyncPhase、SyncError、Ready permit、共通Wire/transfer送信・identity検証 |
| `syncing/host.rs` | HostSyncCoordinator、参加許可、送信transfer、catch-up、finalization |
| `syncing/client.rs` | ClientSyncRouter、受信transfer、baseline/replica適用、finalization |
| `syncing/timing.rs` | 両側共通のresponse/bulk/lifetimeタイマーとprogress |
| `render/mod.rs` | plugin登録、Bevyの順序制約、単一GpuRenderer resourceの所有 |
| `render/buffers.rs` | GPU buffer確保、revisionに応じたupload |
| `render/pipelines.rs` | layout定義、render/compute pipelineのcache・生成 |
| `render/draw.rs` | visibility、sort、main passのindirect draw |
| `render/picking.rs` | point/rectangle target、selection pass、非同期map/readback |
| `render/uniforms.rs` / `extraction.rs` | shader layout・viewport、main Worldからの抽出 |
| `gns/direct_ip/tests.rs` | Direct-IPのcapacity/lifecycleとlocalhostテスト |

Syncの公開型は元の`network::syncing`から再exportします。両側が使うfinalization
candidateとタイマーは共有し、状態の所有者を増やしていません。描画の34関数の本体と
13個のbraced struct定義は、module内visibility・整形を除き移動前と一致します。
shader、scheduleの順序、GPU buffer usage・容量・layout、readbackのsubmit後mapは維持します。
Direct-IPの実装は625行で、既存inbound/outbound/policyを使う一つのbackend所有者として
保ちます。単なる行数削減のための追加分割は行っていません。

## ホストの処理方針

| 方針を表す型 | 対象 | 処理 |
| --- | --- | --- |
| `HostFailurePolicy::Disconnect` | 不正Wire・方向・PlayerId・危険なdelta、Reliableの重複・gap・context不整合 | 対象接続を切断し、後始末を実行 |
| `HostRouteOutcome::Rejected` | Dense StaleTopology、選択可能な対象のないGrab/Rotate | 既存の空ACK・回転結果を保持してReliable配信。control sequenceとauthority cursorを通常どおり進める |
| `HostRouteOutcome::DroppedTransient` | 古いdrag basis、重複・古いtick、まだ到着していない/終了したdrag | `command_drop()`で分類。authority eventを生成しない |
| `HostFailurePolicy::InternalFailure` | 不正なホスト定義、dragの内部不整合、counter枯渇、同期の保持・生成・送信失敗 | ピアを違反として扱わず、対象接続をBackendFailure等で閉じて回復。保持失敗は通常配信を継続。安全に継続できない内部状態はRuntimeFailureとして返す |

部分的に受理したGrab/Rotateは`Applied`です。拒否した部分も既存の結果に含めます。
`SyncError::disconnect_reason()`は同期の切断理由の正本であり、`host_policy()`はその
理由とローカルtransport失敗から方針を導きます。通信の切断理由とUI用の
`NetworkFailureKind`は別の目的を持ち、内部診断は`Internal`で表示側へ渡します。

Reliableの`NoActiveDrag`、`WrongDragContext`、`ActiveDragExists`などは切断を維持します。
これらはcommand sequenceの消費後に起こる場合があり、単なる無応答の拒否では予測や
次のcontrol sequenceが回復しません。既存Wireに安全な拒否結果がないケースを
Transientと同様の破棄へ変更していません。

## 意図的な動作変更

- ホストのローカル状態・保持・エンコードの失敗をProtocol診断からInternalへ変更。
  内部command失敗ではProtocolViolationの代わりにBackendFailureで対象接続を閉じます。
- Bootstrapの切断理由は元の認証拒否・transport失敗の型から選択し、Runtimeで
  一律ProtocolViolationへ置き換えません。
- Gameplay・cancel・Presenceの送信失敗も`HostFailurePolicy::transport()`で切断理由を
  選択します。接続消失やbackpressureはConnectionProblem、ローカルbackendや
  不正な送信サイズなどはBackendFailureで閉じます。従来はすべてConnectionProblemでした。
  いずれもピアのabuse penaltyを増やさず、ReadyクライアントのConnectionLost表示は維持します。
  個別の配信失敗ではホストのfailure/errorを上書きしません。
- 個別ピアの認証拒否、タイムアウト、プロトコル違反はログへ記録し、ホスト全体の
  `NetworkStatus.failure/error`を上書きしません。ホストの内部障害は診断を残します。
- Sync/Bootstrapが先に閉じた接続も共通の後始末へ渡します。cancelが内部エラーを
  返してもRoster削除とpending接続の処理を終えてから返します。安全なcancelを作れない
  内部状態の修復は自動化していません。
- cancelの不整合やauthority eventのエンコード失敗など、全体の整合性を安全に維持できない
  内部障害は`RuntimeFailure::internal`を返します。World側は従来の失敗・teardown経路を使い、
  正本の未配信変更を再適用したり、未複製の修復を行ってホストを継続したりしません。
- ホストAPIから誤ったphaseでrestartする失敗は`HostWrongPhase`として内部障害へ分類。
  ピアからの誤ったphaseのメッセージは従来どおり`WrongPhase`で切断します。
  配信失敗で既に閉じた参加者は、同frameのpump候補から除外します。

## 後続で検討する事項

安全なReliable拒否結果のない操作への新しい応答はWire設計の変更になるため対象外です。
内部状態不整合の自動修復、送信失敗に対する保持済みeventの再送キューも追加していません。
現在の方針は対象ピアを切断して再同期させ、適用済みcommandを再適用しないことです。

## 検証記録

2026-10-11、このWindowsホストで次を確認しました。

- 段階ごとのnetworkテストは分類導入後275件、Runtime・Sync分離後279件成功。
  最終監査で追加した`HostWrongPhase`の回帰テストも成功しています。
- 最終のworkspace通常テスト・doctest（`--all-features -- --skip gns_localhost`）、
  workspaceのアプリビルド（`--all-features`）、default / all-featuresの全target Clippy
  （`-D warnings`）、`cargo fmt --all --check`が成功しました。
- GNS localhostの直列実行はgameの31件とUIのhostname参加1件が成功。
  外部rendezvous binaryを必要とする2件は未実行です。ignoredのGPU image joinは
  次のrelease GPUテストに含めて成功しています。
- 描画分離後のrelease GPUテストはbenchmarkを除いた69件すべて成功。
  Windows / NVIDIA GeForce RTX 5090 / Vulkan（driver 610.88）で、描画・選択・
  buffer upload・非同期readbackと別プロセスのGPU image joinを検証しました。

回帰テストは不正Wire・PlayerId・非有限値・Reliable gapの対象ピアだけの切断、空ACKの
sequence/cursor進行、Transientの破棄、保持失敗時の一度だけの適用とjoin restart、
cancel失敗後のRoster削除・pending切断の完了を明示します。既存のDense競合・同期timeout・
配信失敗・複数ピアのPresence/cancelテストも維持しています。
Linux / macOSの実機動作と性能測定は今回実施していません。
