# Windows ビルド手順

このプロジェクトはWindows環境でのビルドを想定しています。

## 前提条件

1. Rust 1.70以上がインストールされていること
2. Visual Studio 2019/2022 または Build Tools for Visual Studio がインストールされていること

## ビルド手順

1. PowerShellまたはコマンドプロンプトを開く
2. プロジェクトディレクトリに移動
3. 以下のコマンドを実行：

```cmd
cargo build --release
```

## 実行手順

```cmd
cargo run
```

## 既知の問題と対処法

### bevy_renet importエラー
もし以下のエラーが発生した場合：
```
error[E0432]: unresolved imports
```

`src/networking.rs`の3行目を以下のように修正してください：

```rust
use renet::transport::{ServerConfig, ServerAuthentication, ClientAuthentication};
use bevy_renet::transport::{NetcodeServerTransport, NetcodeClientTransport};
```

### gpu_allocator エラー
もしgpu_allocatorに関するエラーが発生した場合、`Cargo.toml`に以下を追加：

```toml
[patch.crates-io]
gpu-allocator = { version = "0.25.0" }
```

## トラブルシューティング

1. **依存関係の問題**: `cargo clean` && `cargo build`を試す
2. **ネットワークエラー**: ファイアウォール設定を確認
3. **パフォーマンス問題**: Release modeでビルド (`cargo build --release`)

## 使用方法

1. アプリケーション起動
2. 「ゲームをホストする」または「ゲームに参加する」を選択
3. パズル画像として `assets/puzzle_image.png` を使用