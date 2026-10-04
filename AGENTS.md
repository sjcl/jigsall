# Repository instructions

## Development guidance

ビルド・ローカル検証・性能計測のコマンドと技術資料の入口は [DEVELOPMENT.md](docs/DEVELOPMENT.md) にまとめています。変更内容に応じて必要なローカル検証を実行してください。README は GitHub の閲覧者向けに機能・技術概要・ビルド方法を簡潔に記載し、宣伝的なコピーは使わないでください。詳しい操作は [PLAYING.md](docs/PLAYING.md)、実装・検証・計測の詳細は docs に記載してください。

描画移行の方針は [INSTRUCTION.md](docs/INSTRUCTION.md)、現在の構成と責務は [ARCHITECTURE.md](docs/ARCHITECTURE.md)、GPU picking の仕様と実機検証は [GPU_PICKING.md](docs/GPU_PICKING.md) を参照してください。過去の移行手順を未実装の機能とみなさず、現行コードを確認してから変更してください。Rust の最低対応バージョンと依存バージョンはルートの `Cargo.toml` を参照し、workspace 共通の依存定義と lockfile を維持してください。

## Architecture invariants

- `puzzella-core` はゲーム状態・命令検証・snap、`puzzella-puzzle` は形状・配置生成、`puzzella-game` は Bevy のライフサイクル・worker・描画、`puzzella-ui` は egui の画面を担当します。core に描画や UI の責務を持ち込まないでください。
- ゲーム状態の正本は CPU の `PieceDataStore` です。入力は命令を発行し、authority が所有権・座標・snap・配置を検証します。入力や GPU から正本を直接変更せず、進捗は個別 Entity の数ではなく正本から求めてください。ネットワーク命令は認証済み player、session、sequence と照合し、client が指定した `placed` を信用しないでください。
- 通常描画は procedural GPU renderer、通常選択は GPU picking を使います。main / point / rectangle で形状・UV・画像 alpha の判定を共有し、元画像の texture を共有してください。ピースごとの Mesh・描画 Entity や旧 batch 再構築方式を通常経路に戻さないでください。CPU 形状・選択の参照実装は feature / test 限定です。
- 入力は現在の egui 処理と camera 更新の後に扱います。非同期選択の古い応答を無視し、release の最終座標を反映してから snap を処理してください。Ctrl / 矩形選択、相対位置を保つ multi-drag、pause / focus loss 時の保持解放を維持してください。
- 生成・画像 decode は worker と channel に分離し、worker から World や GPU resources にアクセスしないでください。ネイティブ画像選択 dialog は非同期で実行し、結果は main thread で適用してください。
- 形状・配置生成は version と seed による再構成を維持してください。異 OS / GPU 間の bit 一致は、検証せずに主張しないでください。

## Performance measurements

性能計測は release モードで行ってください。F3 の性能 overlay と `tracy` / `chrome` feature による tracing を維持してください。60 fps や異 OS での runtime 互換性は、実測・実機検証なしに主張しないでください。

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
