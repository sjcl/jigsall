# 90°単位のcomponent回転

Q / Eで選択中のcomponentを反時計回り / 時計回りに90°回転します。
複数componentはそれぞれ自身の現在position AABB中心をpivotにします。
操作は確定状態に対してだけ適用し、active gesture / drag中は拒否します。
authorityもplaced、disabled、hold、stale topology、混在rotation、不整合な
rigid transformを再検証します。全memberを一括更新し、自動snapは行いません。

## Authoritative invariant

```text
rotation(C) ∈ {0, 1, 2, 3}
position(p) ~= rotate_quarter(correct_position(p), rotation(C)) + translation(C)
translation(C) = position(minimum_member(C))
               - rotate_quarter(correct_position(minimum_member(C)), rotation(C))
```

`rotate_quarter(v, 1) = (-v.y, v.x)`はworld座標の反時計回りです。
CPU/GPUとも符号反転とxy交換だけを使い、sin / cosはありません。
`matches_transform`は従来と同じf32の加減算丸め許容を使い、edge driftを
snap epsilonとして許可しません。`matches_translation`はrotation 0の薄いwrapperです。

回転操作だけmemberを走査してworld AABB中心と新しいcanonical boundsを求めます。
中心とtranslationはf64で計算し、各positionをcanonical座標から再構築します。
すでに丸めたpositionを繰り返し回さず、singletonの中心はbitsまで保持します。
一時planは操作終了時に破棄し、componentやpieceの永続transform storageは追加しません。

## Snap / rendering / persistence

neighbor候補とclosureは同rotationの正しいgrid neighborだけです。
translation比較、stable minimum memberによるtie、固定final transformのclosure、
union順、rounded logical translation cache、境界scanの再利用を維持します。
boardはrotation 0 / translation ZEROであり、0°だけsnapします。
placed memberのrotation bitsは0です。Release中にrotationは変更しません。

CPU/GPU dense stateとsnapshot recordはともに16 bytesのままです。
共通helperがflags bit 9–10をencode / decodeします。
CONNECTED_TOP / RIGHT / BOTTOM / LEFTはcanonical edgeのcacheで、回転時に変更しません。
rendererはworld quadだけ回転し、SDF / UV / profile / outlineはcanonical localを使います。
visibilityとpick visibilityは奇数rotationでAABB extentを交換します。
far splatも長辺を回転し、pixel-center snapping、alpha、depth、pick ROIを維持します。

snapshot schema 4は同じfield orderと16-byte recordでrotationを保存します。
validationはrotation統一、剛体変換、placedのrotation 0と正解positionを検証し、
install時のconnection cacheはDSUから再構築します。ローカルsave codecも同じ
flagsを保存・検証します。

`ProtocolPieceCommand::Rotate`と`RotationCommitted`は既存PieceTarget / ComponentRef /
DenseTargetを使うreliable controlです。受信側は全accepted componentを事前検証し、
同じ決定的計算を適用してrotationを含むresult fingerprintを照合します。
相違は既存のDiverged / trusted snapshot resync経路で処理します。

## Cost and verification

通常frameに追加する処理はGPUのrotation bit decodeと必要なxy交換だけです。
`DragTransform { members, delta }`、O(1) pointer更新、dirty range upload、
component root buffer、idle CPU処理は既存の構造を維持します。
回転時のみ対象componentに比例したCPU処理とstate uploadを行います。

```powershell
cargo test --workspace --lib
cargo test -p puzzella-game --lib render::tests::rotation_tests -- --ignored --test-threads=1
```

CPUテストはsingletonの4000回転、fractional gridの剛体再構成、pair / L字、
独立pivot、hold / topology拒否、rotation-aware snap / closure / drag、
checkpoint / save / snapshot round-trip、reliable authority / replica / wireを検証します。
実GPU fixtureは4方向の色付きUVとSDF、point / rectangle picking、内部edge outlineの
抑制、非正方形AABB、viewport端のfar splatを検証します。

2026-10-03、Windows / NVIDIA GeForce RTX 5090（Vulkan）でworkspace通常テスト
295件とdoctest 1件、既存を含む実GPU回帰15件が通過しました。
`cargo fmt --all --check`とworkspace全targetのClippy（CPU reference / picking
features有効、`-D warnings`）も通過しました。`--all-features`の検査は
GNS native buildのlibclang不足で未完了です。CPU/GPUの速度benchmarkは再計測していません。
