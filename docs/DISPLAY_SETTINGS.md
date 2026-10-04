# 表示設定

表示設定の機能は `game/src/settings.rs` の `DisplaySettingsPlugin` が担当し、egui画面からは `DisplaySettingsAction` を送ります。パズルの状態・セーブデータとは独立しています。

言語設定は同じ `settings.json` の別セクションに保存し、表示設定の適用・取り消しから独立しています。ファイル形式は [SETTINGS.md](SETTINGS.md)、多言語対応、通知の型、フォント、将来のプラットフォーム接続については [LOCALIZATION.md](LOCALIZATION.md) を参照してください。

言語は選択時に適用・保存されます。表示・FPS設定は選択中には保存せず、「適用」を押して反映します。「適用」は有効な変更がある場合だけ有効になり、変更を元に戻すと無効になります。ボーダーレスで使われない解像度の差は変更として扱いません。保存失敗後は同じ設定で再試行できます。

適用が成功したことはボタンの無効化で示し、完了通知は表示しません。設定画面を閉じると編集途中の値と通知をリセットし、未確認の画面変更は取り消します。保存済みの設定と言語の選択は維持し、未解決の保存エラーも再試行のために保持します。

## 技術的な対応

Bevy 0.19.1の `Window` を変更すると、bevy_winitが実行中のOSウィンドウへ反映します。新しい依存エンジンやウィンドウの作り直しは不要です。

| 項目 | 実装 |
| --- | --- |
| ウィンドウサイズ | `WindowResolution::set_physical_resolution`。DPI倍率にかかわらず指定した物理ピクセル数を使う |
| ウィンドウ | `WindowMode::Windowed`。フルスクリーンから戻ると選択したサイズを復元する |
| ボーダーレス | `WindowMode::BorderlessFullscreen`。現在のモニター全体をデスクトップの解像度で覆う |
| フルスクリーン | `WindowMode::Fullscreen`。現在の `Monitor::video_modes` にある解像度を使い、同じ解像度では最も高いリフレッシュレートを選ぶ |
| 最大FPS | `First` のフレーム開始間隔を制御する。前フレームの処理時間を含め、不足時間だけsleepする |
| 無制限 | sleepを解除し、`PresentMode::AutoNoVsync`でリフレッシュレートを超える描画を許可する |

`WinitSettings::Reactive` の待機時間だけでは入力や再描画イベントで更新が発生するため、厳密なFPS上限には使いません。フレーム制限はメニュー・ゲーム・ポーズなどすべてに適用します。無制限でも、実際のFPSはGPU・CPU・OS・ドライバー・利用可能なpresentation modeに依存します。`AutoNoVsync`は未対応環境ではVSyncへフォールバックし、環境によってティアリングが発生します。

BevyのAPI資料: [WindowMode](https://docs.rs/bevy/0.19.1/bevy/window/enum.WindowMode.html)、[WinitSettings](https://docs.rs/bevy/0.19.1/bevy/winit/struct.WinitSettings.html)。実装の確認にはCargo.lockで固定されたローカルのbevy_window / bevy_winitソースも使っています。

## 適用・保存

解像度または画面モードを適用した場合は15秒以内に確認します。取り消し・時間切れで変更前の設定へ戻し、確認されるまでファイルへ保存しません。FPSだけの変更はすぐ保存を要求します。ユーザーデータディレクトリの `jigsall/settings.json`（Windowsでは `%LOCALAPPDATA%\jigsall\settings.json`）の `display` セクションを更新し、他の設定を保持します。共通ワーカーが小さいJSONを一時ファイルへ書き、sync後にatomic replaceで保存します。UI操作はディスク処理の完了を待ちません。確認済みの表示設定は保存待ちで期限を過ぎても取り消さず、保存完了・エラーを非同期に受け取ります。共通の保存処理と終了時の扱いは [SETTINGS.md](SETTINGS.md) を参照してください。

初期値はウィンドウ1280 × 720、最大60 FPSです。最大FPSは10〜1000の整数または無制限です。破損・範囲外の設定ファイルは初期値へ戻します。保存済みのフルスクリーン解像度が現在のモニターで使えない場合はウィンドウへ戻し、設定画面にエラーを表示します。

## 検証

```powershell
cargo test --locked -p jigsall-game settings::tests --lib
# UIなしでOSウィンドウの状態と実測FPSを検証。ユーザーの設定は読み書きしない。
cargo run --locked -p jigsall-game --example display_settings_probe
# 実際の設定画面を描画し、target/settings-ui-*.png を保存する。
cargo test --locked -p jigsall-ui native_settings_ui_probe -- --ignored --nocapture --test-threads=1
```

probeは800 × 600のウィンドウ、ボーダーレス、30 / 120 FPS・無制限、対応解像度のフルスクリーン、ウィンドウへの復帰を順番に実行します。ECSの設定だけでなくwinitの実ウィンドウのサイズ・fullscreen状態を確認し、FPS上限も実時間で検証します。

2026-10-03、UIを作る前にWindowsの通常権限で800 × 600のウィンドウ、2560 × 1440のボーダーレス、1280 × 720 / 240 Hzの排他的フルスクリーン、ウィンドウへの復帰を確認しました。制限付き実行では排他的フルスクリーンのOS呼び出しが `DISP_CHANGE_FAILED` となったため、通常権限でも検証しています。

最終版probeのUIなし・未最適化ビルドでは、上限30で29.76 FPS、上限120で109.84 FPS、無制限で180.15 FPSでした。これは上限の動作確認であり、ゲームプレイの性能保証ではありません。probeではフォーカス状態による計測差を避けるため `WinitSettings::continuous()` を使い、通常のゲームではBevy標準のフォーカス喪失時の省電力更新を維持しています。

設定の保存・再読み込み・破損時の復元・未対応解像度の拒否・時間切れ・明示的な取り消し・保存失敗・フレーム待機・モニター変更・FPS変更時の手動ウィンドウサイズ保持をテストしています。UIはクリックによる無制限の選択と適用、Escによる未適用変更の破棄をテストし、1280 × 720の通常画面と640 × 360の変更確認画面を実描画で確認しました。
