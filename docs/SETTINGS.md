# 設定ファイル

表示、キー割り当て、オートセーブ、言語の設定はすべて、ユーザーデータディレクトリの `puzzella/settings.json` に保存します。Windowsでは `%LOCALAPPDATA%\puzzella\settings.json` です。

```json
{
  "display": {
    "resolution": [1280, 720],
    "mode": "Windowed",
    "max_fps": 60
  },
  "keybindings": {},
  "autosave": {
    "interval_minutes": 5,
    "max_saves_per_game": 1
  },
  "preferences": {
    "language": "auto"
  }
}
```

`keybindings` の省略した操作は初期割り当てを使います。`autosave.interval_minutes` の `null` は無効、`display.max_fps` の `null` は無制限を表します。`autosave.max_saves_per_game` は同じゲームIDのオートセーブを保持する件数（1以上、既定1）です。上限変更は次回のオートセーブ成功後のローテーションに適用します。言語IDは `auto`、`en-US`、`ja` です。

`game/src/settings_file.rs` の `SettingsFile` が各セクションのJSONの読み書きを共通化しています。保存時には最新のファイルを読み直し、対象セクションだけを更新します。一時ファイルに書き込み、sync後にatomic replaceするため、保存失敗で既存ファイルを途中まで書き換えません。

各設定の適用タイミングは独立しています。言語とオートセーブは選択時、キー割り当ては「適用」時、表示設定は変更確認後に保存します。FPSだけの変更は「適用」時に保存します。表示の確認待ち・取り消しは他のセクションへ影響しません。

ファイルやセクションがなければ初期値を使います。セクションの内容が不正なら、その設定にエラーを表示して初期値を使い、他のセクションは読み込めます。ファイル全体が不正なJSONなら、保存もエラーとして既存ファイルを保持します。

旧形式との互換性・自動移行はありません。ルートに表示設定を直接記載した旧 `settings.json` と、旧 `keybindings.json`、`autosave-settings.json`、`ui-settings.json` の設定は読み込みません。
