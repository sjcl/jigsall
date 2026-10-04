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

タイトルの「シングルプレイ → 新しいパズル」から画像とピース数を選びます。
`gns`付きビルドでは「マルチプレイ → 部屋を開く → 新しいパズル / つづきから」でパズルを選び、
「部屋を開いてはじめる」を押すと準備後に接続を受け付けます。
参加者は「マルチプレイ → 部屋に参加」でホスト名またはIPアドレス・ポート（例：example.com:27015）とパスワードを入力します。
game layerのprogrammatic APIは[Direct-IP runtime](DIRECT_IP_RUNTIME.md)を参照してください。

## 実行ファイルのアイコン

Windows向けのビルドでは、ルートの`build.rs`が`assets/icon.ico`を実行ファイルへ埋め込みます。
通常の`cargo build`で適用され、配布時にアイコンファイルを同梱する必要はありません。
Windows以外のターゲットではこの埋め込み処理を行いません。

ウィンドウのタイトルは`Puzzella`です。ウィンドウ生成時に、メニューと共通の
`assets/menu-icon.png`を埋め込み画像から読み込み、タイトルバーとWindowsのタスクバーのアイコンに設定します。
この画像も実行ファイルに含まれるため、起動時の作業ディレクトリや外部のアイコンファイルに依存しません。

元画像は`assets/icon.svg`で、`viewBox`を背景の外周に合わせて透明な余白を除いています。
`icon.ico`はメニューと共通の`assets/menu-icon.png`を元に、16 / 24 / 32 / 48 / 64 / 128 / 256 pxの
画像を格納しています。SVGを更新した場合は、ImageMagickでPNGとICOを再生成してください。
ImageMagickはアイコンの再生成時だけ必要で、通常のビルドには不要です。

```powershell
magick -background none assets/icon.svg -resize 384x384 -depth 8 PNG32:assets/menu-icon.png
magick assets/menu-icon.png -define icon:auto-resize=256,128,64,48,32,24,16 assets/icon.ico
```

## 開発チェック

```powershell
cargo fmt --all --check
cargo check --workspace --locked
cargo clippy --workspace --locked --all-targets --all-features -- -D warnings
cargo test --workspace --locked
```

CIのLinux jobはdefault / all-features両構成のClippy、example、doctest、build、testを検証します。
Windows jobは`gns`を有効にした一構成でworkspaceのunit / integration test、
直列のGNS localhost通信テスト、通常アプリのリンクを検証します。
ファイルの共有・削除や並列ログ出力など、OSによって差が出る実行時の確認を残し、
Clippy、example、doctestと追加のfeature構成はLinux jobに集約しています。

## 依存関係の注意点

Bevy 0.19.1 / bevy_egui 0.42を使用します。Windows向けのwgpu-halはCargo.tomlで29.0.3へ固定しています。29.0.4はgpu-allocator 0.28とWindows COM型が一致しないためです。固定を外す際は、Windowsで再ビルドして互換性を確認してください。

旧Renet試作は削除済みです。Renet importの変更や、gpu-allocatorの古いpatchを追加する必要はありません。

optionalな `gns` feature（`--all-features`も含む）はCMake、Git、libclangとvcpkg経由のnative依存が必要です。通常buildはこれらを要求しません。

## GNSを使うビルド

前提のRust / Visual Studio / CMake / Gitに加えて、`libclang.dll` を含むLLVM 18.1.8をインストールします。

```powershell
winget install --id LLVM.LLVM --exact --version 18.1.8 --source winget
winget pin add --id LLVM.LLVM --exact --version 18.1.8 --source winget
```

GNS 0.3.0が使うbindgen 0.70.1では、LLVM 23.1.2でcallback構造体のフィールドが欠落し、`no field m_eOldState` / `available field is: _address` でビルドが失敗します。Clang 22以降の[bindgenの既知の問題](https://github.com/rust-lang/rust-bindgen/issues/3275)と一致するため、この依存構成ではLLVM 18.1.8を使います。依存更新後に別のLLVMを検証する場合は `winget pin remove --id LLVM.LLVM --exact` で固定を解除できます。`clang.exe --version` だけでなく、`LIBCLANG_PATH` が指す `libclang.dll` も同じ版であることを確認してください。

`%USERPROFILE%\.cargo\config.toml` に次のような設定を追加し、LLVMのインストール先と元リポジトリの場所に合わせてパスを変更してください。`CARGO_HOME` を指定している場合は、そのディレクトリの `config.toml` を使います。既存の設定があれば保持し、`[env]` があればそのテーブルに2項目を追加します。

```toml
[env]
LIBCLANG_PATH = 'C:/Program Files/LLVM/bin'
GNS_VCPKG_BUILDTREES_ROOT = 'C:/work/puzzella/target/vcpkg-trees'
```

vcpkgの作業パスは100文字以内にし、長さチェックは無効化しません。初回のnative buildにはvcpkgと依存ライブラリを取得するためのネットワーク接続が必要です。

Cargoが毎回ユーザー設定を読み込むため、設定後は通常のPowerShellから次のコマンドを実行できます。

```powershell
cargo build --workspace --locked --features gns
# 最適化した実行ファイル
cargo build --locked --release --features gns
# 実際のlocalhost UDP通信を使うテスト
cargo test --locked -p puzzella-game --features gns gns_localhost -- --nocapture
# GNSを含む全featureの開発チェック
cargo clippy --workspace --locked --all-targets --all-features -- -D warnings
```

ユーザー設定は元リポジトリとworktreeの両方に適用されます。`GNS_VCPKG_BUILDTREES_ROOT` は共通の短い作業パスに固定してください。Rustの共有中間ビルドキャッシュ設定は維持し、ロック待ちになったコマンドは先行ビルドの終了までそのまま待ってください。

MSVCのmulti-config generatorでは、依存の最適化設定によりGNSが
`RelWithDebInfo`へ生成されても、GNS 0.3.0のbuild scriptが`Debug`を検索し、
`GameNetworkingSockets_s.lib`のLNK1181が起こる場合があります。
CIはsingle-configのNinjaを使い、libraryを`build/src`直下へ生成します。
既に生成済みのMSVC buildを使う場合、linker出力にあるGNSの`out` directory内の
`lib`をそのコマンドだけ`LIB`検索先へ追加できます。これはRustのtarget/build
directoryやvcpkg作業パスの切り替えではありません。

GNSの依存関係とテストの詳細は[NETWORK_TRANSPORT.md](NETWORK_TRANSPORT.md)を参照してください。

形状生成・入力・状態の設計と今後の課題は[ARCHITECTURE.md](ARCHITECTURE.md)、操作方法は[README.md](../README.md)を参照してください。
