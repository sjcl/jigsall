# Repository instructions

## Commit messages

通常のコミットメッセージは、既存の履歴に合わせて `<type>: <summary>` の形式にしてください。コミット前に `git log -10 --oneline` を確認し、内容に適した種別を選んでください。

- 種別は小文字で記載します。既存の例には `feat`、`fix`、`perf`、`refactor`、`docs`、`test`、`style`、`ui` があります。
- 要約は短い英語で、小文字の動詞から始め、変更内容を具体的に説明します。末尾にピリオドは付けません。
- 接頭辞のない自由形式のタイトルは使いません。
- マージコミットは例外とし、Gitが生成する `Merge ...` の形式を維持します。

例:

```text
feat: redesign puzzle menus with a restrained game theme
fix: keep UI backgrounds behind menus
docs: document commit message conventions
```
