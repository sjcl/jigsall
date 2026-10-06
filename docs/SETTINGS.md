# 設定ファイル

表示、キー割り当て、オートセーブ、画像、言語、プレイヤー表示名の設定はすべて、ユーザーデータディレクトリの `jigsall/settings.json` に保存します。Windowsでは `%LOCALAPPDATA%\jigsall\settings.json` です。

ブランド移行前のユーザーデータディレクトリや設定ファイルからの自動移行は行いません。

設定UIは「一般」「グラフィック」「キーコンフィグ」の3タブです。「一般」は言語とオートセーブを変更時に保存し、プレイヤー表示名は「名前を保存」または入力欄のEnterで検証・保存します。タブ共通の適用ボタンは表示しません。「グラフィック」は表示モード・解像度・最大FPS・ゲーム背景色・擬似3D品質・パズル画像のGPUメモリ予算をまとめます。「適用」は現在のタブの表示設定またはキー割り当てを反映し、画像のメモリ予算は変更時に自動保存します。

```json
{
  "display": {
    "resolution": [1280, 720],
    "mode": "Windowed",
    "max_fps": 60,
    "game_background": "Light",
    "piece_visual_quality": "High"
  },
  "keybindings": {},
  "image": {
    "texture_budget": { "Auto": { "percent": 20 } }
  },
  "autosave": {
    "interval_minutes": 5,
    "max_saves_per_game": 1
  },
  "preferences": {
    "language": "auto"
  },
  "player": {
    "display_name": "日本語 🧩"
  }
}
```

`keybindings` の省略した操作は初期割り当てを使います。`autosave.interval_minutes` の `null` は無効、`display.max_fps` の `null` は無制限を表します。`autosave.max_saves_per_game` は同じゲームIDのオートセーブを保持する件数（1以上、既定1）です。上限変更は次回のオートセーブ成功後のローテーションに適用します。言語IDは `auto`、`en-US`、`ja` です。

## ゲーム画面の背景色

ゲーム画面の背景色は「設定 → グラフィック → ゲーム画面の背景色」で選び、「適用」で反映・保存します。既定はライト（クリーム色、`Light`、sRGB `#F3EAD7`）です。「ダーク」（`Dark`、sRGB `#2B2C2F`）で以前の色に戻せます。既存の `display` に `game_background` がない場合はライトを使い、他の表示設定を保持します。背景色だけの変更に15秒の確認は不要です。

## 擬似3D品質

「設定 → グラフィック → 擬似3D品質」でLow / Medium / Highを選び、「適用」で反映・保存します。
Lowはフラットで軽量、Mediumは控えめな立体表現、Highは影・厚み・面取りを強めに表示します。
既定はHighです。既存の`display`に`piece_visual_quality`がない場合もHighを使い、他の表示設定を保持します。

品質だけの変更は15秒の確認なしで`DisplaySettingsState.current`を更新し、非同期の保存を要求します。
rendererの`PieceVisualQuality` resourceへ同期し、次の描画frameから切り替わります。パズルのreload、
piece state変更、GPU buffer再生成、metadata uploadは行いません。次回起動時には保存した品質を読み込みます。
解像度・画面モードも変更した場合は同じ15秒のpreviewに含まれ、「元に戻す」・timeout・未確認のまま閉じる操作で
品質も以前の値へ戻ります。「変更を維持」で品質も保存します。
Auto、shadow / side / bevelの個別設定、frame-timeによる動的調整は未実装です。
寸法・LODと検証の詳細は[擬似3D描画](PSEUDO_3D.md)を参照してください。

## プレイヤー表示名

`player.display_name` は表示専用のローカル設定です。一般設定とマルチプレイの部屋設定・参加フォームで共通の名前編集UIを使い、「名前を保存」または入力欄のEnterで検証・保存します。どちらも既存の `PlayerSettingsState::commit` と共通の非同期設定保存ワーカーを使います。空欄は `null`（名前なし）になり、UIでは翻訳済みの既定名を表示します。前後のUnicode空白を除去し、32 Unicode scalar / UTF-8 128 bytes以内、control・bidi format文字なしを要求します。入力中はdraftだけを更新し、不正な値は保存せずエラーを表示します。保存した名前は次のoffline puzzleで使用し、Host/Join UIから `HostOptions.display_name` / `JoinOptions.display_name` に渡します。進行中のsessionの名前は変更しません。passwordや接続先secretはこのsectionへ保存しません。

## パズル画像のメモリ予算

画像設定の既定は自動モードで、使用中GPUのメモリ容量の20%を共有RGBA8 textureの予算にします。自動モードの割合は1〜100%で変更できます。手動モードは `"texture_budget": { "Manual": { "mib": 4096 } }` のようにMiBを指定し、UIの範囲は1 MiB〜その端末のGPU容量です。設定を別GPUへ持ち込んだ場合も、decode時に現在の容量以内へ制限します。容量は空きメモリの保証ではなく、他のアプリやパズルのstate buffer等の使用量は差し引きません。

容量はrendererが選んだadapterから取得します。Windows / LinuxのVulkanでは最大のDEVICE_LOCAL heap、WindowsのDirectX 12ではDXGIの専用VRAM（統合GPUでは共有メモリ）、macOSのMetalでは推奨working setを使います。取得できないbackendでは、UIに容量不明を表示し、自動モードは512 MiBをGPUの画像サイズ上限以内へ制限した予算を使います。この場合の手動範囲は、最大textureのRGBA8サイズから計算します。

最大texture辺は `min(16384, device.max_texture_dimension_2d, floor(sqrt(budget_bytes / 4)))` です。正方形でも予算内に収まる保守的な上限で、横長・縦長画像はより少ないメモリを使います。元画像より大きくしません。画像・ゲーム・GPUの辺上限に達した場合、予算を増やしても画質は変わりません。値を下げると拡大時の細部は粗くなりますが、ピース寸法・座標・snap・保存データは変わりません。予算はGPU全体の使用量やdecode用CPUメモリの上限ではありません。

変更時に自動保存し、次の画像選択または保存データのロードから反映します。既にロードした画像のtextureは変更しません。

`game/src/settings_file.rs` の `SettingsFile` が各セクションのJSONの読み書きを共通化しています。起動時の読み込みは同期的ですが、保存要求は共通のバックグラウンドワーカーへ渡し、UI操作ではファイルの読み直し・書き込み・`sync_all`・atomic replaceを行いません。ワーカーは要求順に最新のファイルを読み直し、対象セクションだけを更新します。一時ファイルに書き込み、sync後にatomic replaceするため、保存失敗で既存ファイルを途中まで書き換えません。保存完了・エラーは毎フレーム非同期に受け取り、同じ設定の古い要求に対する応答は無視します。最後の設定リソースを破棄する終了処理では、受け付けた保存を処理し終えるまでワーカーを待ちます。

各設定の適用タイミングは独立しています。言語とオートセーブは選択時、キー割り当ては「適用」時、表示設定は変更確認後に保存を要求します。FPSだけの変更は「適用」時に保存を要求します。言語・オートセーブ・表示設定は保存待ちでも使用できます。キー割り当ては保存成功後に有効化し、失敗時は元の操作を保ちます。表示設定の保存完了通知は実際の成功後だけ表示します。確認済みの表示設定は保存待ちで確認期限を過ぎても取り消さず、ダイアログを閉じた後に遅延した通知や確認画面を再表示しません。表示の確認待ち・取り消しは他のセクションへ影響しません。

ファイルやセクションがなければ初期値を使います。セクションの内容が不正なら、その設定にエラーを表示して初期値を使い、他のセクションは読み込めます。ファイル全体が不正なJSONなら、保存もエラーとして既存ファイルを保持します。

旧形式との互換性・自動移行はありません。ルートに表示設定を直接記載した旧 `settings.json` と、旧 `keybindings.json`、`autosave-settings.json`、`ui-settings.json` の設定は読み込みません。
