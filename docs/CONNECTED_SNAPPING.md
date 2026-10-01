# 永続的なconnected-piece snapping

2026-10-02。基準masterは`ca56bf8`。盤面外でも元画像の上下左右に隣接するピースだけを接続し、以後はcomponent全体を選択・保持・移動・配置します。Ctrlと矩形で複数の独立componentを同時dragする操作も維持します。

作業branchは`codex/connected-piece-snapping`、専用worktreeは`C:\Users\bebe\.codex\worktrees\connected-piece-snapping\puzzella`。開始時にorigin/masterをfetchし、ユーザー提示の`a1d8a38`より新しい`ca56bf8`を基準にしました。開始時の元checkoutや別worktreeの未コミット編集は取り込みませんでした。

master統合時には`1f08f4d`までの4 commitsも維持しました。selection rollback / additive selectionは、自分のholdを許可する最新masterの検証をcomponent全体へ適用し、他playerのGrabは全componentをselectionから除外します。dirty mask allocationの再利用、F3 performance overlay、uncached draw / picking binding修正も維持しています。以下のbenchmarkは単独実装時点（`d770ca7`）の記録です。

統合後も必須6コマンドがすべて成功し、通常 / all-featuresそれぞれ132 tests passed（core24、game83、puzzle24、ui1）。releaseのignored testsも全10件成功（CPU benchmark3件、RTX 5090 GPU test / benchmark7件）。masterで追加された選択整合性・dirty mask再利用のテストに加え、接続componentのlocal / remote holdを含むadditive selectionとrollbackの回帰テストも成功しました。

| 主な変更 | ファイル |
| --- | --- |
| 永続連結・純粋snap計算 | [connectivity.rs](../core/src/connectivity.rs)、[snapping.rs](../core/src/snapping.rs)、[gameplay.rs](../core/src/gameplay.rs) |
| authority・連鎖resolver | [pieces.rs](../game/src/resources/pieces.rs)、[pieces/snapping.rs](../game/src/resources/pieces/snapping.rs) |
| component選択・bulk drag | [interaction.rs](../game/src/interaction.rs) |
| snapshot・disconnect・migration | [snapshot.rs](../game/src/multiplayer/snapshot.rs)、[multiplayer/mod.rs](../game/src/multiplayer/mod.rs) |
| connected回帰テスト | [authority](../game/src/resources/pieces/connected_tests.rs)、[interaction](../game/src/connected_interaction_tests.rs)、[snapshot / migration](../game/src/multiplayer/connected_tests.rs) |
| CPU計測 | [interaction_bench.rs](../game/src/interaction_bench.rs) |

## Representationとauthority

`puzzella-core::PieceConnectivity`はunion-by-sizeのDSUと循環member listです。childの`parent_or_size`はparent ID、負のroot wordの下位20 bitsはsizeです。`next_member`の下位20 bitsはsuccessorで、初期successorは自分自身、unionはrootのsuccessorを交換します。100万という上限で残るbitsへ最小member IDも分割保存し、追加配列なしで安定した座標計算の代表を得ます（root wordに上位11 bits、rootのnext wordに下位9 bits）。unionはpath compressionを使い、read-only root lookupは最悪O(log N)、member iterationはO(k)。component別の恒久Vec / HashMap / Bevy Entityはありません。

| 常駐追加領域 | 1Mのbytes |
| --- | ---: |
| parent_or_size: i32 | 4,000,000 |
| next_member: u32 | 4,000,000 |
| 合計 | 8,000,000 |

GPU stateとsnapshot stateは16 bytes / pieceのまま。connectivityはGPUへuploadしません。dense owners・occupancy・selected / dirty maskも維持します。既存の全選択CPU論理領域24,375,000 bytesにconnectivityを加えた32,375,000 bytesが常駐の目安です。allocator、画像、Bevy、driver、uploadや操作中の一時領域、RSSは別です。

```text
GPU point ID / final rectangle bitset
  → component expansion → GrabGroup
  → authority expansion + whole-component accept/reject
  → component ownership + relative Z
  → frozen drag bitset + pointer delta
  → ReleaseGroup → authority expansion + whole-component owner validation
  → all released components' deltas committed once
  → deterministic resolver
      ├─ neighboring component union + chain frontier
      └─ whole-component board placement
  → dense state / dirty mask / cached placed_count
```

client maskはownership単位として信用しません。部分maskでも全componentへ展開し、全memberのowner / placedを確認します。矛盾したcomponentは部分acceptせず、別componentは独立してacceptできます。Releaseは全componentのdeltaを先にcommitするため、同じgestureの隣接componentを古い位置へsnapしません。処理順はcomponentの最小member ID昇順。吸収済みreleased componentは再処理せず、deltaも二重適用しません。offsetの代表も最小memberなので、snapshot復元でDSU rootが変わっても計算順序と丸めが変わりません。

scalar Grab / Move / Releaseもcomponent全体を処理します。Moveは指定memberがtarget positionになるtranslationを求め、全memberを正解位置とoffsetから再構成します。bulk pointerはpresentationのみ。finite deltaの加算がoverflowする場合は全componentの移動を無視してholdを解放します。低水準`set_state`はsingleton専用です。disconnectもcomponentの全holdを解放し、移動・snap・分裂はしません。既に矛盾した混合ownerが存在する場合はそのcomponentの全ownerを解放します。

## Snapと連鎖

`offset = position - correct_position`。board候補はzero、component候補は正しいgrid neighborのtarget offset。offset差の距離がstrict `< snap_distance`の場合だけ候補になります。距離の二乗はf64で計算してoverflowを避けます。最小距離、同距離ではboard優先、さらにtarget側のboundary PieceId昇順で決定します。HashMap iteration順には依存しません。保持中のtargetへは結合しません。

最初にmoving componentのboundaryを探索し、候補がなければ空間indexを構築しません。候補がある場合だけRelease-localなR-treeへoffsetを登録し、同一offsetはordered PieceId set付きの1点へまとめます。このindexはtranslation候補の検索に使い、GPU picking・readback・shaderは変更しません。

targetを吸収する前にそのmember listを一度探索し、新しいboundary候補を登録します。union後に探索すると成長中のcomponentを再走査してしまうため、その順序を避けます。indexにはtarget rootごとに最小boundary IDを1件保存し、吸収時のcandidate削除にもmember走査は不要です。既存候補もindexに残し、offsetが変われば距離を再評価します。遠いtargetの全memberは走査せず、実際に結合する候補になった時だけ全ownership / translationを検証し、Release単位のbitsetに結果をcacheします。

連鎖中は論理offsetと正解座標boundsだけを更新します。boundsでfiniteな再構成をO(1)検証し、一時member Vecに新たな未配置memberを集め、完了後に一度`correct_position + canonical_offset`へ正規化します。boardへ固定されたcomponentはzero offsetを維持し、zero offsetの隣接componentとも連結します。Release内で探索を完了した配置済みcomponentを再利用する場合、その大componentのboundaryやstateを再走査・再書込しません。board近くのまだずれたcomponentは自身のReleaseでboardへsnapします。

f32加減算では任意のoffsetを全memberでbit単位に一致させられません。再構成で相対形状の誤差を丸めの範囲へ抑え、snapshotは各componentの同じ代表offsetに対して再構成を検証します。許容幅は座標scaleの4 × f32 epsilonで、edgeごとの誤差を足し合わせません。placed positionsは正解座標との完全一致を要求します。

## Selection・dirty・Z

point readbackは最大1 PieceId。CPUが全componentへ展開し、Ctrlも全memberを追加／解除します。final rectangle readbackは従来のbitsetで、一部memberへのhitを全componentへ展開します。遅延readback・additive rectangle・rollbackも現在のconnectivityとownershipで再検証します。矩形preview自体は従来のGPU hit maskです。

drag startでcanonical membershipを固定し、pointer updateはdeltaだけ変更します。Grab / ReleaseのClientCommand数は各1。Grabとfront移動はcomponent内と複数選択全体のrelative Zを維持し、MAX_Zでのrare compactionも残します。placementは全memberを正解位置・placed・holdなし・Z zeroへ更新します。

dirtyは変化したdense memberだけ。同じ位置でunionしたtargetはconnectivityだけが変わるのでstate upload不要。selectionは別maskだけ更新します。full dirtyのdense copy 1個、fragmented dirtyの最大128 rangesから1 spanへの縮約、単一変更16-byte rangeを維持します。

## Snapshot schema 3

| flags | bit |
| --- | ---: |
| SNAPSHOT_PLACED | 0 |
| SNAPSHOT_CONNECTED_RIGHT | 1 |
| SNAPSHOT_CONNECTED_DOWN | 2 |

captureは右・下のneighborが同じcomponentならedgeを保存します。root IDをwireへ保存しません。installはedgeからDSUを再構成し、schema、session / image / cursor / definition、count、finite position、Z、unknown flags、境界外edge、placedの誤座標、component内のplaced / translation不一致を全て検証してからstoreを置き換えます。schema 1 / 2は拒否します。

restoreはposition / Z / placed / connectivityを維持し、holds / selection / dragをresetし、GPU epochを更新します。local drag中のcapture拒否とmigrationのtransactional activationを維持します。

## 計算量とMILLION_SELECTIONからの変更

Nはpiece count、kは展開member数、eは探索した最大4kのgrid edges、bは発見したboundary target ID数。bitsetの初期化・iterationにはO(N/32)があります。全piece stateを毎操作走査する処理はありません。

| 経路 | 維持／変更 |
| --- | --- |
| idle / unfocused idle | O(1)、全component走査なし、state / mask upload 0 |
| pointer | O(1)、position更新・command・state / membership upload 0 |
| point GPU | 最大1 ID / 4 bytes。CPU expansion O(k)追加 |
| rectangle preview | GPUのみ、readbackなし |
| rectangle final | O(N/32) readback、CPU expansionとcomponent検証O(k)追加 |
| mask clone / rollback | Arc共有、connectivity変更後のrollbackのみ再展開 |
| Grab | expansion / validation + 既存Z sort O(k log k)、1 command |
| Release | resolverごとにO(k + e)のmember / edge処理 + Release共通O(N/32) + 候補index操作、1 command |
| union | path compression + union-by-size、list splice O(1) |
| snapshot | explicit O(N)にDSU再構成追加、16 bytes維持 |
| GPU / dirty | renderer / shader変更なし、既存upload境界維持 |

candidate indexは通常O(log b)のinsert / nearest検索。同一offsetの大量tieは1点へまとめます。異なるoffset点が多数同距離になる敵対的な配置では、同距離点数に応じたtie検証が残ります。1つの連鎖resolver内ではmember / edgeを定数回程度で処理し、配置済みtargetはRelease全体でも再走査を抑えます。別resolverが既に解決した未配置componentを異なるoffsetへ再吸収する場合は再訪問するため、任意のoffset分布についてRelease全体のworst-case線形性までは保証しません。scratch masksはRelease全体で一度確保し、componentごとにN-bit maskをzero初期化しません。一時root / pending-member Vecは各最大約4 MB、4個のscratch bitsetは計約0.5 MB（1M時）。他にcanonical masks、候補Vec、spatial index等が明示的な操作中のみ存在し、恒久8 MBには含みません。

## Benchmarkとvalidation

release / locked、4096² image、seed 42、1k / 10k / 100k / 1M、各5 samples、pointer各100,000 updates。`connected_snapping_cpu_benchmark`は未連結、全体1 component、1 memberのReleaseからの連鎖結合、交互offsetの全体board配置の4 casesを分け、metadata初期化・union chain・iteration・expansion・Grab・pointer・Releaseを測定します。未連結fixtureは隣接offset差をthresholdより大きくして、Release後も未連結であることをassertします。board fixtureはgridのcheckerboardでoffset ±4を与え、近いboardへの配置後に大componentが成長する経路も測ります。GPU / upload / frame wall timeをCPU authority時間へ含めません。

baselineは基準masterのソースを専用worktreeのtarget内へ展開し、同じ未連結fixture・pointer暖機条件の計測testだけを追加して実行します。既存million-selectionとmulti-drag benchmarkも再実行します。

計測環境はWindows 11、Ryzen 9 9950X、Rust 1.97.0、RTX 5090 / Vulkan / NVIDIA 610.88。[環境JSON](../benchmarks/connected-snapping-environment.json)、[新CPU CSV](../benchmarks/connected-snapping-cpu.csv)、[baseline CSV](../benchmarks/connected-snapping-baseline-release.csv)に条件と全samplesを保存しています。以下は各5回の中央値、Releaseはmsです。

| pieces | master未連結 | 新未連結 | 単一component | 1 memberからの全体連鎖 | checkerboard全体board配置 | 接続済みpointer ns |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1,000 | 0.0164 | 0.1097 | 0.0375 | 0.1378 | 0.5868 | 11.679 |
| 10,000 | 0.1628 | 1.0906 | 0.3746 | 1.4902 | 6.1306 | 11.774 |
| 100,000 | 1.6382 | 11.1304 | 3.8558 | 15.4165 | 62.4450 | 11.863 |
| 1,000,000 | 17.1576 | 112.9137 | 38.1059 | 162.9509 | 963.6666 | 11.780 |

100万の単一componentではmetadata初期化1.0377 ms、union chain構築4.3551 ms、member iteration1.1342 ms、1 memberから全体へのexpansion4.5171 ms、Grab11.6614 ms。未連結Releaseは正しい隣接探索の追加で約6.58倍になりました。連鎖は1 memberのReleaseから100万memberを接続するまでを含みます。board 1M samplesは627.7–1041.7 msでばらつきもあるため、frame latencyが小さいとは解釈しません。いずれもpanic / OOMなし、owner解放と最終component size / placementをassertしています。

既存[million-selection CPU再計測](../benchmarks/connected-snapping-million-selection-cpu.csv)もfixtureを変更せず通りました。1Mのselection final commit1.6540 ms、pointer11.842 ns、Release1466.8007 ms、upload準備2.4618 ms。この既存fixtureの`position = (id, 10000)`は、新ルールでは隣接offsetが近くなって多数の連鎖結合を起こすため、従来の「単体を盤面へ判定するRelease」と処理内容が変わります。純粋な未連結のbefore / after比較は上表の同一fixtureを使います。[multi-drag CSV](../benchmarks/connected-snapping-multi-drag.csv)も1k–1Mで11.854–12.005 ns / pointer frame、commandはGrab / Release各1です。

実GPUの[million-selection CSV](../benchmarks/connected-snapping-million-selection-rtx5090.csv)は2048²、rectangle GPU中央値0.521216 ms、readback125,000 bytes、selection upload125,000 bytes、selection時state upload 0。既存testはsamples間でpositionだけをresetするため、run 0は未連結から全体結合、run 1–4は永続的な1M componentです。接続済みrun 1–4のRelease中央値40.0 ms、CPU finalization中央値7.274 ms。pointer中のstate / drag-mask / selection uploadはすべて0をassertしました。[既存renderer再計測CSV](../benchmarks/connected-snapping-renderer-rtx5090.csv)は24 rows、1M全体viewのframe2.2984 ms、半透明2.3001 ms。shader・rendererの変更はありません。

baselineを再現する場合、`ca56bf8`の隔離checkoutへ[baseline harness](../benchmarks/connected-snapping-baseline-harness.rs)を`game/src/baseline_release_bench.rs`としてコピーし、`game/src/interaction.rs`へ`#[cfg(test)] #[path = "baseline_release_bench.rs"] mod baseline_release_bench;`を追加して、`cargo test -p puzzella-game --release --locked connected_baseline_release_benchmark -- --ignored --nocapture --test-threads=1`を実行します。baselineと新実装には別のtarget directoryを使ってください。

```sh
cargo fmt --check
cargo check --locked
cargo clippy --workspace --locked --all-targets --all-features -- -D warnings
cargo test --locked
cargo test --locked --all-features
cargo build --locked
cargo test -p puzzella-game --release --locked connected_snapping_cpu_benchmark -- --ignored --nocapture --test-threads=1
cargo test -p puzzella-game --release --locked million_selection_cpu_benchmark -- --ignored --nocapture --test-threads=1
cargo test -p puzzella-game --release --locked multi_drag_cpu_benchmark -- --ignored --nocapture --test-threads=1
cargo test -p puzzella-game --release --locked gpu_ -- --ignored --nocapture --test-threads=1
cargo test -p puzzella-game --release --locked render_only_image_upload_keeps_metadata_and_pixel_values -- --ignored --nocapture --test-threads=1
```

上記6つの必須検証はすべて成功。通常とall-featuresの各testは122 passed / 0 failed / 11 ignored（core24、game74、puzzle24）。ignoredもreleaseで全て実行し、CPU benchmark3件、RTX 5090実GPU test / benchmark8件が成功しました。strict threshold、誤隣接、component同士の連鎖、部分mask / 矛盾owner、scalar、Z compaction、selection / rollback、dirty range、snapshot境界 / 旧schema / atomic reject、migration後の全体drag、100万member round tripを検証しています。DSU rootとmember listの順が異なる復元前後で整数・fractional offsetのsnap結果が同じことも確認しています。

回転・分裂、transport実装・途中dragのpeer presentation同期、異OS / GPU検証、process RSS測定は含みません。f32 rounding、spatial indexの敵対的tie cost、異なるresolverによる未配置componentの再訪問、explicitな大selection / release / snapshotのCPU costは残ります。idle / pointerのO(1)、GPU pickingとbitset readback、各1 ClientCommand、dirty upload境界は維持できましたが、Releaseの実時間、selection finalization、mainのmetadata初期化とCPU常駐memoryは増えています。
