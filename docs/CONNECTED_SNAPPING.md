# 永続的なconnected-piece snapping

2026-10-02。最新master `664f7bee9e3a06749fbf7c4eb00145c617212e86`をfetchして、branch `codex/simplify-connected-snapping`、専用worktree `C:\Users\bebe\.codex\worktrees\simplify-connected-snapping\puzzella`でresolverを簡略化しました。既存checkout、並行branch、masterには変更を加えていません。

## Releaseの仕様

`offset = position - correct_position`。1回のReleaseで各moving componentのsnap translationを変更できるのは最大1回です。全選択componentのRelease deltaを先にcommitし、最小member PieceId順に独立して判定します。

```text
Release deltaを全componentにcommit
  ↓
release直後のoffsetを確定
  ↓
boardのstrict threshold内ならboardを優先 → final offset = ZERO
  または、正しいgrid neighborの最良targetへ1回だけsnap
  候補なしならrelease直後のoffsetを維持
  ↓
moving componentをcorrect_position + final offsetへ正規化
  ↓
final offsetを固定
  ↓
同final-offset neighborとのunion closure
  ↓
終了
```

board判定は `distance < snap_distance`。等号ではsnapしません。boardまで3、neighborまで1でもboardへ配置し、neighborとの候補比較は実行しません。board snapは全memberのpositionを正解座標、placedをtrue、holdを解除、Zを0にします。ZERO offsetですでに整列しているvalidな隣接componentも結合し、未配置memberがあれば同時に配置します。配置済みtargetはZEROから動きません。

board範囲外ではmoving componentの各memberの上下左右、最大4 neighborだけを調べます。別component、全memberがenabled・unheld、placementとtranslationがcomponent内で整合しているtargetのみ受け入れます。offset距離はf64の二乗で計算し、strict threshold内の最小距離、tieならtarget componentの最小member PieceIdで決めます。DSU root IDやHashMapのiteration順はtieに使いません。同じtarget rootは一度だけ評価し、遠いtargetの全member検証は行いません。

snap後のfinal offsetはimmutableです。追加unionはtargetの代表offsetとfinal offsetの**完全一致**を要求し、近いだけのcomponentは吸収しません。例えばfinal offset `(100, 50)` に対して `(101, 50)` や `(104, 50)` は結合しません。targetの位置を別offsetへ正規化する処理はありません。

`A/B/C`のx offsetが`0/4/8`、thresholdが5の場合、boardを範囲外にする共通y offset 100を与えたauthority testで、AはBへ1回だけsnapし、ABのx offsetは4、Cは未接続のままであることを検証します。文字どおりのZERO offsetならboard優先によりAはboardへ配置されます。同じfinal offsetのcomponentを結合するclosureは許可され、新たに露出する正しいgrid boundaryも探索します。

## Resolverとscratch

`game/src/resources/pieces/snapping.rs`には単一candidateの選択、moving componentの正規化、固定offset closureだけを残しました。旧`CandidateIndex`、Release-local R-tree、`BTreeMap`、`BTreeSet`、candidate Vec、nearest再探索、translation更新loop、`CorrectBounds::merge`、`completed_placed`を削除しました。closure用member queueは移動先を再決定する用途には使いません。

incoming componentのmember listをunionの前にqueueへ追加し、成長するcomponentの全memberを毎回探索しません。Release内で解決済みのrootは一般化した`resolved` bitsetへ保存し、後続componentがそのtargetへsnapしてもtargetの境界・positionは再走査・再書込しません。absorbed released componentも再解決しません。追加unionによって先に解決したcomponentのtranslationは変わりません。

scratchの4 bitsets（seen targets、validated、eligible、resolved）はRelease全体で一度だけ確保します。component切替ではseen targetsのtouched rootsだけをclearします。touched Vecとclosure member Vecはcapacityを再利用し、disconnected singletonの候補重複除去はstack上の4 root配列で行い、seen bitsetの書込も省きます。singletonごとのtree/map/Vecの新規allocationはありません。1M時のscratch bitsetsは合計500,000 bytes、queueとtouched Vecはそれぞれ最大約4 MB、別にRelease root Vec等の操作中領域があります。恒久connectivityは8,000,000 bytesのままです。process RSSとallocator overheadは計測していません。

`rstar`はCPU collision / picking debugのため維持しますが、通常runtimeの必須依存からoptional dependencyへ変更し、`cpu-picking-debug` featureとdev-dependenciesだけで有効化します。Releaseにはspatial indexもordered treeもありません。Cargo.lockの変更は不要でした。

## 座標計算

`PuzzleDefinition::correct_position()`は`PuzzlePiece`を構築しません。Release-local `PuzzleGeometry`にgrid寸法、piece size、centerを一度計算し、PieceIdのinteger division / moduloとmultiply / subtractで座標を求めます。generatorと同じ演算順序、特にyの積の後の符号反転を維持します。1×1、fractional size、非正方grid、1M gridと最大image寸法について全pieceの座標bitsが従来`piece().correct_position`と一致するテストを追加しました。neighbor計算も同じgrid helperを共有し、一時heap生成はなく、左右のrow wrapはありません。

f32の加減算では任意offsetを全memberでbit単位に保存できません。componentの代表は最小memberで、shape検証はその代表に対する再構成との差を座標scaleの4 × f32 epsilon以内に限定します。closureのoffset equalityにこの許容幅は使いません。placed positionsは正解座標との完全一致を要求します。

## Authority・selection・snapshot

部分maskのcanonical expansion、全componentのownership accept/reject、mixed ownerの全体reject、scalar Grab / Move / Releaseのatomicity、remote holdのtarget拒否、disconnectの全component hold解放を維持します。複数の独立componentは1 GrabGroup / ReleaseGroupで操作でき、各componentは独立してsnapします。targetは静止し、同じgesture内の他componentが再translationされることはありません。

point / Ctrl / final rectangleはcomponent全体へ展開し、union時は必要な未選択componentへだけselectionを伝播します。remote hold除外、rollbackとdelayed readbackのauthority再検証を維持します。rectangle previewは従来のGPU hit maskで、GPUにconnectivityを追加していません。pointerはfrozen drag maskを共有しdeltaだけを更新します。dense dirty uploadとrelative Zの既存境界も維持します。

snapshot schema 3、`SNAPSHOT_CONNECTED_RIGHT` / `SNAPSHOT_CONNECTED_DOWN`、16 bytes / pieceは変更しません。root IDを保存せず、右・下edgeから復元します。invalid border edge、inconsistent component、placed exact position、old schema reject、transactional install、migration round tripを維持します。integer / fractional offsetでDSU rootが変わる復元と、新resolverの単一snap・closure・board優先の復元前後一致をテストします。

## 計算量

Nはpiece count、kはmoving membersと新たに吸収する未解決members、eはその最大4k grid edges、vはcandidate targetのauthority検証membersです。component resolverはmember走査・normalization・closureがO(k + v)、read-only DSU lookupが最悪O(e log N)、unionはpath compressionを使います。Release共通bitset allocation / iterationはO(N/32)、accepted memberの列挙・root lookupも必要です。root検証はcacheし、unionしたrootには検証済みの状態を引き継ぎます。各memberの境界・placement・selection変更はRelease内で定数回程度に抑え、成長componentの繰り返し全走査を避けます。pointerとidleはO(1)です。

## Benchmark

比較対象は**今回fetchした旧resolver付きmaster** `664f7bee9e3a06749fbf7c4eb00145c617212e86`です。専用worktreeのignored target内へ`git archive`で隔離したソースに、新しいbenchmark harnessだけを適用しました。connected snapping導入前のcommitとの比較は行っていません。統合直前のfirst parentは`1f08f4d131eaf510ada8c321e3ca2baf0d25304e`で、過去の`ca56bf8`計測を今回のbaselineには使っていません。

release / locked、4096² image、seed 42、1k / 10k / 100k / 1M、各scenario 5 samples、pointer 100,000 updates。両実装で同じharnessを使い、baseline計測後にworkspace 3crateのrelease cacheを削除し、新実装をbuildし直して追加テストが実行されることを確認してから順次計測しました。再現時はbaselineと新実装で別target directoryを使ってください。setup・Grab・union構築・pointer・Releaseの時間は別々に記録し、authority Release時間にGPU / upload / frame wall timeを含めません。

| scenario | fixtureとassert |
| --- | --- |
| disconnected | correct position + `(10000 + id * 20, 10000)`、全piece Release、結合なし |
| single_snap | 0 / 1だけoffset `(10000, 10000)`、他は遠いoffset、0をdelta `(1, 1)`でRelease、size 2 |
| closure | 全targetがoffset `(10000, 10000)`、0をdelta `(1, 1)`でRelease、固定offsetのclosureでsize N |
| board | grid checkerboardのoffset `(±4, 0)`、全piece Release、全piece placed / size N |
| connected | offset `(10000, 10000)`、事前に全Nをunion、全component Release |

旧benchmarkの実fixtureはすでに同一offsetでの結合だったため、名称を`closure`へ明確化しました。translationを変えて遠方まで移動するbenchmarkはありません。各fixtureでreleased / placed / owner解放 / component sizeをassertします。pointerはmembership Arcとdense state pointerを固定し、state dirtyなしと各transition 1 commandをassertします。

baseline再現は`664f7be`の隔離checkoutへ今回の`game/src/interaction_bench.rs`だけをコピーし、上記のconnected CPU benchmarkを実行します。旧`ca56bf8`専用baseline harnessは削除しました。過去CSVは当時の記録で、今回の比較表には使いません。

計測値・環境・validationは下記に今回の結果を記載します。

計測環境はWindows 11 Pro 10.0.26200、Ryzen 9 9950X、Rust 1.97.0、RTX 5090 / Vulkan / NVIDIA 610.88。[環境JSON](../benchmarks/connected-snapping-environment.json)、[新CPU CSV](../benchmarks/connected-snapping-cpu.csv)、[旧master CPU CSV](../benchmarks/single-snap-old-master-cpu.csv)に全samplesとprovenanceを保存しています。以下は5回の中央値で、Releaseはmsです。

新resolver:

| pieces | disconnected | single snap | same-offset closure | board | already-connected | connected pointer ns |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1,000 | 0.0515 | 0.0012 | 0.0453 | 0.1272 | 0.0304 | 11.615 |
| 10,000 | 0.5060 | 0.0046 | 0.4749 | 1.2789 | 0.2925 | 11.851 |
| 100,000 | 5.2783 | 0.0228 | 4.7237 | 13.5733 | 2.9183 | 11.732 |
| 1,000,000 | 52.2758 | 0.4322 | 45.8704 | 131.5409 | 28.8865 | 11.530 |

旧master（同じfixture）:

| pieces | disconnected | single snap | same-offset closure | board | already-connected |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 1,000 | 0.1120 | 0.0030 | 0.1409 | 0.6175 | 0.0370 |
| 10,000 | 1.0792 | 0.0084 | 1.4422 | 6.3026 | 0.3674 |
| 100,000 | 10.9321 | 0.0149 | 14.9232 | 63.5571 | 3.6569 |
| 1,000,000 | 109.9164 | 0.2968 | 161.2626 | 905.7347 | 44.3986 |

100万の比較:

| 1M scenario | 旧master ms | 新resolver ms | 短縮率 |
| --- | ---: | ---: | ---: |
| disconnected | 109.9164 | 52.2758 | 52.4% |
| single_snap | 0.2968 | 0.4322 | -45.6% |
| closure | 161.2626 | 45.8704 | 71.6% |
| board | 905.7347 | 131.5409 | 85.5% |
| connected | 44.3986 | 28.8865 | 34.9% |

disconnectedはstack上のsingleton候補重複除去も含めて約2.10倍、boardは約6.89倍、already-connectedは約1.54倍速くなりました。same-offset closureは約3.52倍です。小さなReleaseのsingle snapは1M puzzle上で旧0.2968 ms → 新0.4322 msと遅く、100kでも旧0.0149 ms → 新0.0228 msでした。少数memberのReleaseでもN-bit masksの確保・初期化が残るため、小操作のコストはpiece countに依存します。これは今回残る性能課題です。single snapの1M全samplesは新0.4057–0.5556 msで、allocationと短時間測定のばらつきもあります。

既存[million-selection CPU](../benchmarks/connected-snapping-million-selection-cpu.csv)も同じfixtureで成功しました。1Mのfinal commit 1.6226 ms、Release 128.8948 ms、pointer 11.863 ns、upload準備 2.1377 msです。fixtureの`position = (id, 10000)`では隣接snapが発生するため、上表の純disconnectedとは異なります。旧仕様に依存した最後のpieceの位置assertを、全pieceがrelease直後からthreshold未満しか動かないassertへ変更しました。

[multi-drag CSV](../benchmarks/connected-snapping-multi-drag.csv)は1k / 10k / 100k / 1Mで12.585 / 11.968 / 11.885 / 11.878 ns。pointer中はmembership Arcとdense state pointerが変わらず、dirtyなし、Grab / Release各1 commandを維持しました。

実GPUの[million-selection CSV](../benchmarks/connected-snapping-million-selection-rtx5090.csv)は2048²、rectangle GPU中央値 0.516608 ms、readback / selection upload各125,000 bytes、selection時state upload 0、pointer中state / membership / selection upload 0をassertしました。初回は未連結、run 1–4は永続的な1M componentです。run 1–4のauthority Release中央値 100.4497 msでした。[既存renderer CSV](../benchmarks/connected-snapping-renderer-rtx5090.csv)も24 rowsを保存しました。shader / renderer / GPU picking / connectivity uploadの変更はありません。

必須6コマンドは最終コードですべてexit 0。通常とall-featuresそれぞれ140 passed / 0 failed / 10 ignored（core25、game90、puzzle24、ui1）。releaseのcore/game通常testsは115 passed、CPU benchmark3件、実GPU test / benchmark7件もすべて成功しました。strict threshold、no chained translation、same-offset closure、board priority、wrong neighbor、独立multi-release、remote / mixed ownership、placed target、selection / rollback、dirty range、snapshot境界 / schema / root independence、migration、100万member round tripを含みます。

| 変更ファイル | 内容 |
| --- | --- |
| `core/src/gameplay.rs`, `core/src/lib.rs` | precomputed geometry、generatorと座標bitsの一致テスト、neighbor helper |
| `core/src/snapping.rs` | boardとneighborの新しい役割に合わせてcandidate説明・旧tie testを整理 |
| `game/src/resources/pieces.rs`, `pieces/snapping.rs` | Release共有scratch、単一snap、固定offset union、singleton fast path |
| `game/src/resources/pieces/connected_tests.rs`, `game/src/multiplayer/connected_tests.rs` | 新仕様とauthority / snapshotの回帰検証 |
| `game/src/interaction_bench.rs` | 5 scenariosと全pieceの移動上限検証 |
| `game/Cargo.toml` | rstarをCPU picking用optional dependencyへ変更 |
| `docs/CONNECTED_SNAPPING.md`, `docs/ARCHITECTURE.md` | 現仕様・計算量・比較・再現手順 |
| `benchmarks/connected-snapping-*.csv`, `single-snap-old-master-cpu.csv`, environment JSON | CPU / GPU / rendererの実測更新、同じfixtureのbaseline保存 |
| `benchmarks/connected-snapping-baseline-harness.rs` | 旧baseline専用の不要harnessを削除 |

`core/src/connectivity.rs`、`game/src/interaction.rs`、`game/src/multiplayer/snapshot.rs`のproduction codeは変更していません。snapshot wire format・multiplayer・selectionの既存invariantを検証で維持しました。


## 再実行

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

残る課題はexplicitな大selection / Grab / Releaseのmember列挙、最大4 edges / memberのCPU探索、authority validationとsnapshotのO(N)処理です。board優先でも全memberの配置・dirty更新・unionは必要で、frame latency全体の保証ではありません。回転・分裂、transport、異OS / GPU、RSSは今回の変更に含みません。
