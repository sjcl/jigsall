# 永続的なconnected-piece snapping

2026-10-05追記: 旧 v2 の CPU メッシュ生成・CPU picking と専用 feature / example は削除済みです。以下の旧実装・比較コマンドは当時の記録です。現行の構成と検証コマンドは [DEVELOPMENT.md](DEVELOPMENT.md) を参照してください。

2026-10-04追記: 初回リリースのsnapshot schemaは1（開発時schema 5と同じlayout）、generatorはv1（開発時v5と同じ生成結果）です。本書の旧schemaやgenerator番号は開発時の記録で、互換decoderは提供しません。現在の構成は[ARCHITECTURE.md](ARCHITECTURE.md)を参照してください。

2026-10-02。`4b1f01de5db1247fdc627c156d6840e74673f8e7`で単一translationのresolverをmasterへ統合しました。続いて、そのmasterを基準にbranch `codex/rounded-closure-scratch`でf32丸めを含むclosure判定と少数Releaseのscratchを改善しました。2026-10-03に90°単位の剛体回転へ拡張しました。以下は現行仕様です。

## Releaseの仕様

`rotation = decode_rotation(flags)`、`offset = position - rotate_quarter(correct_position, rotation)`。1回のReleaseで各moving componentのsnap translationを変更できるのは最大1回です。全選択componentのRelease deltaを先にcommitし、最小member PieceId順に独立して判定します。

```text
Release deltaを全componentにcommit
  ↓
release直後のoffsetを確定
  ↓
rotation == 0でboardのstrict threshold内ならboardを優先 → final offset = ZERO
  または、同rotationの正しいgrid neighborの最良targetへ1回だけsnap
  候補なしならrelease直後のoffsetを維持
  ↓
moving componentをrotate_quarter(correct_position, rotation) + final offsetへ正規化
  ↓
final offsetを固定
  ↓
同rotation・同final-offset neighborとのunion closure
  ↓
終了
```

board判定は `rotation == 0 && distance < snap_distance`。等号ではsnapしません。boardまで3、neighborまで1でもboardへ配置し、neighborとの候補比較は実行しません。board snapは全memberのpositionを正解座標、placedをtrue、holdを解除、Zを0にします。ZERO offsetですでに整列しているvalidな隣接componentも結合し、未配置memberがあれば同時に配置します。配置済みtargetはZEROから動きません。

board範囲外ではmoving componentの各memberの上下左右、最大4 neighborだけを調べます。別component、全memberがenabled・unheld、movingと同rotationで、placementとrigid transformがcomponent内で整合しているtargetのみ受け入れます。offset距離はf64の二乗で計算し、strict threshold内の最小距離、tieならtarget componentの最小member PieceIdで決めます。DSU root IDやHashMapのiteration順はtieに使いません。同じtarget rootは一度だけ評価し、遠いtargetの全member検証は行いません。

snap後のfinal offsetはimmutableです。追加unionはtargetの最小memberのpositionを `matches_transform(position, correct_position, rotation, final_offset)` で検証します。f32の加減算による丸めを許容するだけで、snap_distanceによる再snapは行いません。代表offsetのbitsがfinal offsetと異なる場合は、targetの全memberも同じ固定final offsetに対して検証し、edgeごとに誤差を蓄積しません。例えばfinal offset `(100, 50)` に対して `(101, 50)` や `(104, 50)` は結合しません。board配置を除きtargetのposition / Z / flagsは書き換えません。

`A/B/C`のx offsetが`0/4/8`、thresholdが5の場合、boardを範囲外にする共通y offset 100を与えたauthority testで、AはBへ1回だけsnapし、ABのx offsetは4、Cは未接続のままであることを検証します。文字どおりのZERO offsetならboard優先によりAはboardへ配置されます。同じfinal offsetのcomponentを結合するclosureは許可され、新たに露出する正しいgrid boundaryも探索します。

## Resolverとscratch

`game/src/resources/pieces/snapping.rs`には単一candidateの選択、moving componentの正規化、固定offset closureだけを残しました。旧`CandidateIndex`、Release-local R-tree、`BTreeMap`、`BTreeSet`、candidate Vec、nearest再探索、translation更新loop、`CorrectBounds::merge`、`completed_placed`を削除しました。closure用member queueは移動先を再決定する用途には使いません。rotationもRelease中は固定し、board配置ではrotation bitsを0へ正規化します。

incoming componentのmember listをunionの前にqueueへ追加し、成長するcomponentの全memberを毎回探索しません。Release内で解決済みのrootは`resolved` scratch setへ保存し、後続componentがそのtargetへsnapしてもtargetの境界は再走査せず、positionも再書込しません。異なる丸めbitsのlogical offsetを比較する際は、固定final offsetに対する全member検証が必要です。absorbed released componentも再解決しません。追加unionによって先に解決したcomponentのtranslationは変わりません。

scratchの4 membership sets（seen targets、validated、eligible、resolved）は `PieceScratchSet` を使います。128 IDsまでは固定長のstack配列（512 bytes / set）へ保存し、容量を超えたsetだけN-bitのdense wordsへ遅延昇格します。denseへ昇格した後も、clearは非zeroだったwordだけをresetしcapacityを再利用します。canonical expansion / validationの重複除去も同じ構造です。closure member Vecはcapacityを再利用し、disconnected singletonの候補重複除去はstack上の4 root配列で行います。1M上の1 / 32 member snapでは4 setsのheap allocationが0 bytesであることをtestしています。大量処理だけdense storageを確保し、1Mで各昇格setのwordsは125,000 bytes、touched-word indicesは最大31,250個です。closure queueとRelease root Vecは実際に列挙したmember / component数に比例します。恒久connectivityは8,000,000 bytesのままで、全piece向けの世代番号配列は追加しません。process RSSとallocator overheadは計測していません。

代表のpositionからoffsetを復元するとbitsが変わる解決済みrootについてだけ、Release-local HashMapへ固定logical offsetを保存します。これにより検証済みcomponentのoffsetが丸めで変わった扱いになることを避けます。mapはkey lookupだけに使い、tieやroot処理順は引き続き最小member PieceIdで決めます。mapが空のままならheap確保せず、ZEROのboard配置ではlogical offsetの記録は不要です。Release終了時に破棄します。snapshot schema 4は同じ16-byte layoutのflags bit 9–10にrotationを保存します。

`rstar`はCPU collision / picking debugのため維持しますが、通常runtimeの必須依存からoptional dependencyへ変更し、`cpu-picking-debug` featureとdev-dependenciesだけで有効化します。Releaseにはspatial indexもordered treeもありません。Cargo.lockの変更は不要でした。

## 座標計算

`PuzzleDefinition::correct_position()`は`PuzzlePiece`を構築しません。Release-local `PuzzleGeometry`にgrid寸法、piece size、centerを一度計算し、PieceIdのinteger division / moduloとmultiply / subtractで座標を求めます。generatorと同じ演算順序、特にyの積の後の符号反転を維持します。1×1、fractional size、非正方grid、1M gridと最大image寸法について全pieceの座標bitsが従来`piece().correct_position`と一致するテストを追加しました。neighbor計算も同じgrid helperを共有し、一時heap生成はなく、左右のrow wrapはありません。

f32の加減算では任意offsetを全memberでbit単位に保存できません。componentの代表は最小memberで、shape検証はその代表に対する再構成との差を座標scaleの4 × f32 epsilon以内に限定します。closureも同じhelperで、固定したfinal offsetに対する再構成を検証します。画像幅4096・3×1・logical offset 100.37のように端pieceの復元offsetだけが100.369995へ丸められても、targetを移動せず結合できます。placed positionsは正解座標との完全一致を要求します。

## Authority・selection・snapshot

Releaseはrequested maskから最小member順のcomponent rootsを取り出し、rootごとに全memberのownershipとplacedを検証します。部分maskでもcomponent全体を処理し、accepted maskの再構築は不要です。scalar Releaseはrootを直接渡すためN-bit requested maskも作りません。部分maskのcanonical expansion、全componentのownership accept/reject、mixed ownerの全体reject、scalar Grab / Move / Releaseのatomicity、remote holdのtarget拒否、disconnectの全component hold解放を維持します。複数の独立componentは1 GrabGroup / ReleaseGroupで操作でき、各componentは独立してsnapします。targetは静止し、同じgesture内の他componentが再translationされることはありません。

point / Ctrl / final rectangleはcomponent全体へ展開し、union時は必要な未選択componentへだけselectionを伝播します。remote hold除外、rollbackとdelayed readbackのauthority再検証を維持します。rectangle previewはGPU direct hit maskをcomponent rootのmaskへcollapseし、hitしたcomponent全体を表示します。final readbackはdirect hitのままCPU authorityで展開します。root bufferはunion-by-sizeでabsorbed memberだけdirtyにし、upload時に最終rootを取得します。pointerはfrozen drag maskを共有しdeltaだけを更新します。dense dirty uploadとrelative Zの既存境界も維持します。

snapshot schema 4は`SNAPSHOT_CONNECTED_RIGHT` / `SNAPSHOT_CONNECTED_DOWN`とrotation bitsを保存し、16 bytes / pieceを維持します。root IDを保存せず、右・下edgeから復元します。invalid border edge、inconsistent component、placed exact position、old schema reject、transactional install、migration round tripを維持します。integer / fractional offsetでDSU rootが変わる復元と、新resolverの単一snap・closure・board優先の復元前後一致をテストします。

## 接続componentの選択outline

選択outlineの接続辺cacheは既存16-byte `GpuPieceState.flags` のbit 5 / 6 / 7 / 8へtop / right / bottom / leftを保存します。authorityは引き続き`PieceConnectivity`です。固定offsetのclosureが既に列挙する正しいgrid neighborについて、union成立時と同じrootの辺を訪問した時に両側のbitを設定します。後者は2×2などの閉路の共有辺も記録します。bitが変わったpieceだけ既存dirty maskへ追加し、成長componentの追加走査・恒久allocation・GPU buffer・root ID uploadはありません。解決済みtargetのmember listも再走査しません。

main fragmentは4辺のdistanceを一度計算し、全辺のmaxを従来どおりcoverage / discardへ使います。黄色selectionと青いcomponent previewは同じboundary関数で接続済み辺を候補から除外します。全4辺が接続したpieceにはoutlineがありません。point / rectangle pickingは従来の全辺SDFを使います。形状定数・fingerprint・generator versionは変更しません。

cacheは結合処理とsnapshot installだけで更新し、selection変更・idle・camera・pointer dragに再計算も追加state uploadもありません。新しい2piece接続は両側の16-byte state、計32 bytesをdirty uploadします。既存のposition / hold変更は同じdirty bitへ合流します。大量closureの瞬間には多数の新しい接続辺がdirtyになりますが、uploadのrange結合と128 spans超での既存fallbackを維持します。snapshot schema 4のcaptureはrotationを保存し、connected render flagsは保存せずDSUからright/downを導出します。installは既存state生成のmap内で復元DSUの隣接関係から全4方向を再構成し、保存edgeに明示されない共有閉路辺も復元します。追加DSU lookupはこの明示的なO(N)復元時だけです。

CPU testsは横 / 縦pair、未接続neighborとgrid外周、2×2の閉路・L字・穴あり・全4辺接続、10k closureの既存境界走査数、解決済みboard clusterへの接続、snapshotのcache非保存と疎な閉路辺からの復元、1M restore、2stateだけの32-byte upload、idle / frozen dragでstate allocationとupload revisionが変わらないことを検証します。実GPUの`gpu_connected_selection_outlines_preserve_coverage_picking_and_uploads`はsingleton / 横 / 縦 / 2×2 / L字 / 穴あり / 内部pieceについて、境界から離れたpixelの画像色、外周outline、coverageとpoint / rectangle pickingの不変、camera / drag中のstate / membership / selection upload 0 bytesを検証します。既存`gpu_raster_selection`はsingletonの色とprocedural shape parity、`gpu_drag_transform_and_preview_without_readback`は青previewの既存表示も確認します。

## 計算量

Nはpiece count、kはmoving membersと新たに吸収する未解決members、eはその最大4k grid edges、vはcandidate targetのauthority検証membersです。component resolverはmember走査・normalization・closureがO(k + v)、read-only DSU lookupが最悪O(e log N)、unionはpath compressionを使います。scalar Releaseのscratch初期化はNに依存せず、Group Releaseでは入力maskのiterationにO(N/32)が残ります。少数IDのdeduplicationは固定容量内で線形検索し、それを超えるsetだけO(N/32)でdenseへ昇格します。Releaseはrequested IDsのroot lookupとsort、component単位のauthority検証・member列挙を行います。Grabとselectionではcanonical出力maskのCOWが必要な場合があり、scalar GrabもN-bit入力maskを作ります。root検証はcacheし、unionしたrootには検証済みの状態を引き継ぎます。各memberの境界・placement・selection変更はRelease内で定数回程度に抑え、成長componentの繰り返し全走査を避けます。pointerとidleはO(1)です。

## 初回resolver簡略化のBenchmark（664f7be → 4b1f01d）

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

disconnectedはstack上のsingleton候補重複除去も含めて約2.10倍、boardは約6.89倍、already-connectedは約1.54倍速くなりました。same-offset closureは約3.52倍です。この初回実装では、小さなReleaseのsingle snapは1M puzzle上で旧0.2968 ms → 新0.4322 msと遅く、100kでも旧0.0149 ms → 新0.0228 msでした。少数memberのReleaseでもN-bit masksの確保・初期化が残るため、小操作のコストはpiece countに依存します。この課題への追補結果は次節に記載します。single snapの1M全samplesは新0.4057–0.5556 msで、allocationと短時間測定のばらつきもあります。

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

初回resolver簡略化では`core/src/connectivity.rs`、`game/src/interaction.rs`、`game/src/multiplayer/snapshot.rs`のproduction codeは変更していません。snapshot wire format・multiplayer・selectionの既存invariantを検証で維持しました。


## f32丸めと少数Releaseの改善（4b1f01d基準）

比較対象は改善直前のmaster `4b1f01de5db1247fdc627c156d6840e74673f8e7`です。ignored targetへarchived sourceを用意し、新しいsmall-operation harnessのみを追加してbaselineを計測しました。両方のtimed fixtureとassertは同一です。baselineの計測後にworkspace cratesのrelease artifactsをcleanし、追加testが含まれるbinaryを確認してから最終実装を計測しました。

新fixtureは1 / 8 / 32 memberのcomponentと隣の静止targetを同じoffsetへ置き、他pieceを遠くへ置きます。Groupは中央member 1個だけの部分maskとdelta (1,1)をReleaseし、scalarはMove後にReleaseします。初期化・union・Grab・Move・mask作成は測定外、authority applyのReleaseだけを測定します。released数、sizeがmembers+1、全owner解放、target位置不変を毎sample assertします。各条件10 samples、合計240 samplesの中央値（µs）です。

| pieces | members | Group baseline | Group 改善後 | scalar baseline | scalar 改善後 |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 1000 | 1 | 1.20 | 0.40 | 0.80 | 0.30 |
| 1000 | 8 | 1.55 | 1.00 | 1.60 | 0.90 |
| 1000 | 32 | 3.60 | 2.50 | 3.50 | 2.40 |
| 10000 | 1 | 3.60 | 0.65 | 4.25 | 0.40 |
| 10000 | 8 | 4.85 | 1.30 | 5.05 | 1.00 |
| 10000 | 32 | 7.10 | 2.85 | 7.00 | 2.75 |
| 100000 | 1 | 6.70 | 2.45 | 6.85 | 2.35 |
| 100000 | 8 | 8.45 | 3.80 | 8.95 | 2.15 |
| 100000 | 32 | 10.90 | 4.85 | 10.95 | 4.35 |
| 1000000 | 1 | 289.75 | 14.25 | 372.40 | 7.60 |
| 1000000 | 8 | 306.55 | 16.70 | 329.35 | 8.35 |
| 1000000 | 32 | 261.65 | 20.25 | 336.80 | 11.20 |

1MのGroup Releaseは1 / 8 / 32 membersで約20.3 / 18.4 / 12.9倍、scalarは約49.0 / 39.4 / 30.1倍速くなりました。[baseline CSV](../benchmarks/rounded-small-release-baseline.csv)と[改善後CSV](../benchmarks/rounded-small-release-cpu.csv)に全samplesがあります。

既存connected-snapping harnessも両実装で再実行しました。5 samplesの1M中央値（ms）です。small-operation fixtureと異なりselection / frozen dragを準備してからReleaseし、Group入力maskの走査や共有maskのCOWも含みます。

| scenario | baseline ms | 改善後 ms |
| --- | ---: | ---: |
| disconnected | 54.0945 | 51.7415 |
| single_snap | 0.3398 | 0.0431 |
| closure | 46.4607 | 46.9471 |
| board | 131.3834 | 94.0195 |
| connected | 30.1446 | 24.9713 |

single_snapは0.3398 → 0.0431 ms、boardは131.3834 → 94.0195 ms、already-connectedは30.1446 → 24.9713 msです。大量closureは46.4607 → 46.9471 ms（約1.0%増）でした。offset bitsが一致する場合はeligibility検証が同じfixed offsetを保証するので、代表の重複matches_translationを省きます。[bulk baseline](../benchmarks/rounded-release-baseline.csv)、[改善後bulk CSV](../benchmarks/rounded-release-cpu.csv)、[環境・検証JSON](../benchmarks/rounded-closure-environment.json)に記録しています。

1Mの既存[million-selection CPU](../benchmarks/rounded-million-selection-cpu.csv)と[multi-drag](../benchmarks/rounded-multi-drag.csv)も成功し、pointerは約12 nsを維持しました。[実GPU selection](../benchmarks/rounded-million-selection-rtx5090.csv)と[renderer CSV](../benchmarks/rounded-renderer-rtx5090.csv)では、pointer中のuploadなし、selectionのstate uploadなし、transitionごと1 command、pixel値の既存assertを維持しました。million-selection CPUのfixtureは独立offsetが混在するため、上のbulk fixturesとは異なります。

最終コードの必須6コマンドはすべてexit 0。通常・all-featuresはそれぞれ147 passed / 0 failed / 11 ignored（core26、game96、puzzle24、ui1）。release core/gameは122 passed、ignored CPU4件・実GPU7件も成功しました。fractional closureの両axis、target position bits不変、100.38の別translation拒否、全target memberのfixed-offset整合性、復元後一致、1M上の1 / 32 memberでscratch mask heap 0 bytes、10k fractional clusterの境界1走査を新規検証しています。

残るN依存の処理はGroup入力bitmapの走査、共有drag / selection maskを変更する場合のCOW、scalar Grabの入力mask、canonical出力maskです。今回除去したのは少数Releaseの不要なN-bit scratch初期化とaccepted mask再構築であり、full frame latencyや全操作O(k)を保証するものではありません。snapshot schema、16-byte state、8-byte / pieceの恒久DSU、pointer pathは変更していません。

## 再実行

```sh
cargo fmt --check
cargo check --locked
cargo clippy --workspace --locked --all-targets --all-features -- -D warnings
cargo test --locked
cargo test --locked --all-features
cargo build --locked
cargo test -p puzzella-game --release --locked small_component_release_cpu_benchmark -- --ignored --nocapture --test-threads=1
cargo test -p puzzella-game --release --locked connected_snapping_cpu_benchmark -- --ignored --nocapture --test-threads=1
cargo test -p puzzella-game --release --locked million_selection_cpu_benchmark -- --ignored --nocapture --test-threads=1
cargo test -p puzzella-game --release --locked multi_drag_cpu_benchmark -- --ignored --nocapture --test-threads=1
cargo test -p puzzella-game --release --locked gpu_ -- --ignored --nocapture --test-threads=1
cargo test -p puzzella-game --release --locked render_only_image_upload_keeps_metadata_and_pixel_values -- --ignored --nocapture --test-threads=1
```

残る課題はexplicitな大selection / Grab / Releaseのmember列挙、最大4 edges / memberのCPU探索、authority validationとsnapshotのO(N)処理です。board優先でも全memberの配置・dirty更新・unionは必要で、frame latency全体の保証ではありません。回転・分裂、transport、異OS / GPU、RSSは今回の変更に含みません。
