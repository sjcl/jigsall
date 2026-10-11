# ゲーム内通知とイベント履歴

操作は [遊び方](PLAYING.md)、ビルド・検証は [開発ガイド](DEVELOPMENT.md)を参照してください。

## 責務と保持期間

`game/src/events.rs` の `GameEvent` は Bevy のローカル `Message` です。
ゲーム側が確定した状態遷移から発行し、`ui/src/notifications.rs` が履歴・翻訳・描画を担当します。
通信プロトコル、checkpoint、セーブ形式には追加しません。

UI は構造化したイベントと表示時刻を `VecDeque` に最大200件保持します。
名前はイベントにコピーするため、Rosterから退出者を削除した後も履歴で表示できます。
表示時に Fluent を適用し、履歴を開いたまま言語を変更しても内容を翻訳し直します。
表示時刻はローカルの壁時計、トーストの4秒の期限は `std::time::Instant` です。

通常トーストは最新3件を保持し、進行中は接続IDまたはオートセーブの操作キーで別に保持します。
進行中と通常通知を合わせた同時表示は最大3件です。集中時に隠れた進行状態も完了まで保持し、
新しい通知が消えた後に再表示できます。履歴を開いている間はトーストを描画しません。
履歴上限やトースト期限によって、進行中の処理を完了とみなすことはありません。

## 発行タイミング

| イベント | 発行箇所・条件 |
| --- | --- |
| `PlayerJoining` | ホストの `host_sync_event` で認証済み接続を `joining` に登録し、同期を開始するとき |
| `PlayerJoined` | ホストの同期・Ready・Roster更新が成功した後の `host_joined`。既存クライアントは連続revisionのPresence適用成功後 |
| `PlayerJoinCancelled` | 同期中の接続を確定した切断処理で `joining` から削除したとき。runtime全体の失敗・終了でも残った操作を終了 |
| `PlayerLeft` | ホストの `remove_ready` 成功時、クライアントのPresence適用成功時。両方とも削除前に名前を確保 |
| `AutosaveStarted` | PostUpdateで保存要求を取り出し、実際のcheckpoint captureを開始するとき |
| `AutosaveSucceeded` | 現在のgenerationの保存ワーカー結果が成功し、ローテーションエラーもないとき |
| `AutosaveFailed` | capture・workerへの送信・保存ワーカー結果の失敗。処理中のworker応答channel切断も対象 |
| `AutosaveRotationFailed` | 保存本体は成功し、ワーカー結果の `rotation_error` があるとき。成功通知と別々に表示せず1件の警告にする |
| `SessionReset` | オフラインゲームの新規開始・ロード、ネットワークdriverの導入・明示的セッション終了、Menuでのゲームcleanup後 |

認証前の接続、初期RosterSnapshot、クライアント自身のReadyでは参加通知を発行しません。
進行中の参加通知はホストだけのローカル情報です。重複切断は既存のlive接続削除で抑制し、
Presenceは既存のrevision検証が成功したときだけ通知します。保存結果は既存のgeneration照合後に通知します。
一時停止・完成への移行では履歴を初期化しません。

## 描画と入力

HUDの実際の下端を使い、右上のトーストと履歴を配置します。トーストはeguiのpainterだけで描き、
入力を取得するAreaやwidgetを作りません。履歴は高さに上限のあるスクロール可能なWindowで、
タイトルバー・スクロールバー・閉じる操作も既存の `GameUiPointerCapture` とeguiの入力判定に含めます。
通知処理はpiece/storeを参照せず、piece数に比例する走査・GPU upload・Entity生成を追加しません。

新しい通知を追加する場合は、`GameEvent` にvariantを追加し、確定した処理結果から発行します。
UIのメッセージ・種別・重要度と両言語のFluentキーを追加し、進行通知なら開始・終了で共有する操作キーを定義します。

## 検証

通知専用テストはnetwork runtimeのPresence fixture、persistence runtimeのworker/storage fixture、
UIの通知テストにあります。通常のworkspaceテストで実行します。

```sh
cargo fmt --all --check
cargo test --workspace --locked --features rendezvous -- --skip gns_localhost
cargo clippy --workspace --locked --all-targets --features rendezvous -- -D warnings
```

UIテストは入力透過、履歴のクリック・ドラッグ・スクロールのcapture、日英の描画、
履歴上限・期限・複数同時操作・resetを確認します。native window / GPUや性能計測は通常のunitテストに含みません。

非表示のnative windowとGPUを使う通知専用プローブもあります。

```sh
cargo test --workspace --locked --features rendezvous native_notification_ui_probe -- --ignored --test-threads=1
```

スクリーンショットはworktreeの `target/ui-probes/notifications-{ja,en}-{toasts,history}.png` に出力します。
2026-10-11、このWindowsホストでプローブを実行し、日英のトースト（最大3件）の配置・警告/エラーの区別・
履歴パネルを目視確認しました。これは描画の確認であり、100万ピースでの性能計測ではありません。
