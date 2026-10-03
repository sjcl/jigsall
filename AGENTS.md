# Repository instructions

## Rust builds

Rust の中間ビルドキャッシュは、元リポジトリと worktree の間で共有する設定を使用します。

- Cargo がビルドディレクトリのファイルロック待ちになった場合は、先行ビルドが完了してロックが解放されるまで、そのコマンドを維持して待ってください。
- 順番待ちを回避する目的で、`--target-dir`、`CARGO_TARGET_DIR`、`CARGO_BUILD_BUILD_DIR`、Cargo の `build.target-dir` / `build.build-dir` などを変更し、別のビルドキャッシュを作成しないでください。
- 順番待ちを理由に、他のビルドプロセスを停止したり、ロックファイルを削除したりしないでください。

## Windows GNS builds

このWindowsホストでは、GNS用のLLVM/libclang 18.1.8をインストールし、Cargoのユーザー設定（`%USERPROFILE%\.cargo\config.toml`）に `LIBCLANG_PATH` と `GNS_VCPKG_BUILDTREES_ROOT` を設定しています。GNSのbindgen 0.70.1はLLVM 23と互換性がないため、検証済みの18.1.8を使用してください。通常のPowerShellから、環境変数の追加設定なしで実行できます。

```powershell
cargo build --workspace --locked --features gns
cargo test --locked -p puzzella-game --features gns gns_localhost -- --nocapture
```

`--all-features` も同じ設定を使います。設定は元リポジトリとworktreeの両方に適用されます。GNSのvcpkg作業パスは元リポジトリの `target/vcpkg-trees` を使用し、worktreeごとのパスへ切り替えないでください。詳細と別ホストでのセットアップは [Windowsビルド手順](docs/WINDOWS_BUILD.md) を参照してください。

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
