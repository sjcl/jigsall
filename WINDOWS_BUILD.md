# Windows ビルド手順

## 前提

- Rust 1.95以上（MSVC toolchain）
- Visual Studio 2022 Build Toolsの「C++によるデスクトップ開発」とWindows SDK
- Bevyの描画backendに対応するGPU / driver

PowerShellでリポジトリへ移動し、lockfileを使用して実行します。

```powershell
cargo run --locked
# 最適化した実行ファイルを作る場合
cargo build --locked --release
.\target\release\puzzella.exe
```

メニューの「Game Setup」から画像とパズルサイズ・seedを設定してください。通信のHost / Joinは未実装です。

## 開発チェック

```powershell
cargo fmt --check
cargo check --locked
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked
```

## 依存関係の注意点

Bevy 0.19.1 / bevy_egui 0.42を使用します。Windows向けのwgpu-halはCargo.tomlで29.0.3へ固定しています。29.0.4はgpu-allocator 0.28とWindows COM型が一致しないためです。固定を外す際は、Windowsで再ビルドして互換性を確認してください。

旧Renet試作は削除済みです。Renet importの変更や、gpu-allocatorの古いpatchを追加する必要はありません。

形状生成・入力・状態の設計と今後の課題は[ARCHITECTURE.md](ARCHITECTURE.md)、操作方法は[README.md](README.md)を参照してください。
