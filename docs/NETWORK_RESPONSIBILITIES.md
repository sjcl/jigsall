# ネットワークの責務と失敗方針

Wire形式と互換性番号は変更していません。ゲーム状態の正本・sequence tracker・
authority cursorの所有者も維持しています。

## Runtimeの境界

- `runtime.rs`：transport poll、roleごとの処理呼び出し、画像decode、ローカルcommand、teardown。
- `runtime/lifecycle.rs`：接続イベントとbootstrap gate、live集合による一度だけの後始末。
- `runtime/host.rs`：Syncの開始・進行、Gameplayのapply→retain→publish、Presence、切断時cancel。
- `runtime/client.rs`：Syncの結果適用、replicaと予測状態のreconcile。

送信は全Readyピアへ試行してから失敗した接続を後始末します。切断の連鎖はqueueで
処理し、cancelの適用やPresenceのrevision更新はlive集合からの削除だけが所有します。
保持失敗はcatch-upのrestartを進めつつ通常配信を継続し、commandを再適用しません。

## ホストの処理方針

| 方針を表す型 | 対象 | 処理 |
| --- | --- | --- |
| `HostFailurePolicy::Disconnect` | 不正Wire・方向・PlayerId・危険なdelta、Reliableの重複・gap・context不整合 | 対象接続を切断し、後始末を実行 |
| `HostRouteOutcome::Rejected` | Dense StaleTopology、選択可能な対象のないGrab/Rotate | 既存の空ACK・回転結果を保持してReliable配信。control sequenceとauthority cursorを通常どおり進める |
| `HostRouteOutcome::DroppedTransient` | 古いdrag basis、重複・古いtick、まだ到着していない/終了したdrag | `command_drop()`で分類。authority eventを生成しない |
| `HostFailurePolicy::InternalFailure` | 不正なホスト定義、dragの内部不整合、counter枯渇、同期の保持・生成・送信失敗 | ピアを違反として扱わず、回復に必要な接続をBackendFailure等で閉じる |

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
- 個別ピアの認証拒否、タイムアウト、プロトコル違反はログへ記録し、ホスト全体の
  `NetworkStatus.failure/error`を上書きしません。ホストの内部障害は診断を残します。
- Sync/Bootstrapが先に閉じた接続も共通の後始末へ渡します。cancelが内部エラーを
  返してもRoster削除とpending接続の処理を終えてから返します。安全なcancelを作れない
  内部状態の修復は自動化していません。

## 後続で検討する事項

安全なReliable拒否結果のない操作への新しい応答はWire設計の変更になるため対象外です。
内部状態不整合の自動修復、送信失敗に対する保持済みeventの再送キューも追加していません。
現在の方針は対象ピアを切断して再同期させ、適用済みcommandを再適用しないことです。
