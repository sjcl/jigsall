# Repository instructions

## CI and local validation

通常の自動検証は [GitHub Actions CI](.github/workflows/ci.yml) に任せます。PR、`master` への push、手動実行で、`Cargo.toml` の `workspace.package.rust-version` から読み取った最低対応バージョンの Rust を使い、以下の検証を行います。

- `cargo fmt --all --check`
- Windows / Linux で、通常構成と `--all-features` 構成の workspace 全体の Clippy（全 target、警告をエラー化）、テスト・doctest、ビルド
- GNS の localhost 通信テスト（全 feature 構成で直列実行）

CI と同じ結果しか得られない検証をローカルで繰り返さないでください。通常の `cargo check`、`cargo clippy`、`cargo test`、`cargo build`、`cargo fmt --check` は CI に任せ、変更ごとの確認や完了報告のためだけに実行しません。回帰テストの追加・修正は行い、実行結果は CI で確認します。整形が必要な場合の `cargo fmt --all` は実行できます。

ローカルでは、変更に関連し、CI では同じ結果を得られない検証だけを必要な範囲で行います。対象は実 GPU を使う ignored テスト、ネイティブウィンドウ・UI 操作、実機固有の問題、release の性能・メモリ計測などです。CI の失敗原因を調べるために必要な最小限の再現・修正確認、またはユーザーが明示的に依頼した検証は実行できます。その場合は CI との重複が必要な理由を説明し、全チェックを一律に再実行しないでください。

CI をまだ実行していない、または結果を取得できない場合は、その事実を報告してください。ローカルで重複検証して埋め合わせたり、CI が成功したと扱ったりしません。

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

通常のコミットメッセージは、既存の履歴に合わせて `<type>: <summary>` の形式にしてください。

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
