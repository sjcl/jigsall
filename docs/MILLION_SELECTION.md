# Million-piece selection / bulk interaction

2026-10-02。branch `perf/million-piece-selection`、基準 `014cab45fa61be5535cf857d62cabbe48204aeb8`。専用worktreeは `C:\Users\bebe\.codex\worktrees\million-selection\puzzella`。refreshしたorigin/masterから開始し、元workspaceの未コミット変更には触れていません。途中のmasterの変更も取り込んでいません。

## 変更前の調査

| 項目 | 基準commitの実装 |
| --- | --- |
| PieceDataStore / selected_pieces | dense 16-byte stateと別のHashSet<PieceId> |
| previous_selected | HashSet。symmetric_differenceを別HashSetへcollectし、clone_from |
| rollback | pointer downでselected HashSetをclone |
| rectangle final | GPU atomic bitset → async staging / RawResult bytes → decode_ids Vec → さらにfilter Vec → selection HashSet |
| preview | GPU専用bitset → fragment outline。CPU readbackなし |
| selected highlight | Lastで各pieceのSELECTED flagを更新しdirty stateをupload |
| drag start | Z順のID Vecをgestureに保持。N個のGrab / ClientCommand |
| pointer | 固定GPU membershipとdrag_delta。すでにO(1) |
| drag release | N個のMove + N個のRelease、その後各pieceの完了Messageとsnap |
| held_by | HashMap<PieceId, PlayerId> |
| authority boundary | coreのPieceCommand / ClientCommand、serialized ClientCommandEnvelope。player / reliable control / move sequence / authority epochの検証 |
| migration / snapshot | schema 2、dense position / Z / PLACEDだけを共有。active local dragをcapture時に拒否。installでselection / holds / presentationをreset |
| benchmarks | release CPU pointer 1k / 10k / 100k / 1M、実GPU near / medium / entire、opaque / translucent、同じ4サイズ |

## 変更後の型とフロー

`core::PieceBitSet`はrow-major ID用の小さな型です。`Arc<[u32]>`、bit_len、cached countを持ち、contains / insert / remove / clear / fill / union / difference / xor / retain / set-bit iterationを提供します。1Mは31,250 words = 125,000 bytesです。cloneはO(1)でword allocationを共有し、変更が必要な場合だけcopy-on-writeします。全memberが有効なretainも共有を維持します。

selected_piecesとdirty_piecesをbitsetへ変更し、previous_selectedは削除しました。gestureのoriginalもbitsetです。ID Vecはgestureに保持しません。`PieceOwners`はdense PlayerId配列とoccupancy mask、playerごとのcached hold countを持ちます。全u64 PlayerIdが有効で、sentinel IDは予約しません。owner allocationは最初のholdで遅延確保し、bulk operationのplayer accountingはgroup単位で行います。

`SelectionPayload::Point(Option<PieceId>)`と`Rectangle(PieceBitSet)`を分離しました。pointは4 bytes、final rectangleは4 × ceil(N / 32) bytesを受信し、wordのままmaskに変換します。寸法・末尾paddingを検証し、clipped / empty ROIのcopyを伴わない空応答は空maskへnormalizeします。latest requestと一致しない応答は従来どおり破棄します。GPU応答後に変わったplaced / ownershipをCPU authorityで再検証してからmaskをcommitし、additive selectionはoriginal maskとのunionです。

```text
rectangle final GPU bitset → RawResult bytes → PieceBitSet
  → current authorityによる再検証 → committed selected mask
  → Arc共有によるextract → selected GPU bitset → fragment outline
```

selection変更はdense stateのflagsもdirty stateも更新しません。fragmentはselectedとpreviewを別bufferから読み、黄色のselectedを優先します。PLACED / HELD / ENABLEDは引き続きpiece stateです。previewは従来のGPU passだけで、staging / readback / CPU mask commitを行いません。

```text
drag start:
  selected mask → selectable membership → GrabGroup 1個
  → authority再検証 → 相対Z順の一時sort → dense owners / HELD / Z
  → accepted membershipをGPUへ一度upload

pointer:
  有限なpointer → drag_delta uniformだけ更新

release / pause / focus loss:
  frozen mask + last displayed delta → ReleaseGroup 1個
  → 各memberのowner検証 → position + delta → hold解放
  → 各pieceのsnap → placed_count → cached progress / completion
```

`PieceCommand::{GrabGroup, ReleaseGroup}`はcoreのserialized command境界にあります。`PieceDataStore::apply_command`がまとめて処理し、`AppliedCommand`は受理 / released / placedの集計だけを返します。各memberのRust command / ClientCommand / PieceMoveCompleted / PiecePlacedEventは生成しません。単一pieceの旧commandも維持し、同じsnapを適用します。snap閾値は従来どおりdistance < snap_distanceです。無効owner、placed、非有限delta、異なるmask寸法を拒否し、mixed ownershipではそのplayerのmemberだけをreleaseします。位置加算だけがoverflowする場合は、旧Move→Releaseと同じくMoveを無視してholdを解放します。

Z順を保つsort用の一時VecはGrabのauthority処理中だけ残ります。sorted unique IDを使ってZを更新し、24-bit上限を越える前に従来のrare compactionを行います。releaseはmaskをiterateするだけです。

所有者が変わったmemberはlocal dragの表示maskから除外します。全memberが拒否されたGrabも表示maskを消すため、別playerのholdにlocal deltaを適用しません。通常のpointer更新はこの検証を繰り返さず、authority変更時だけ処理します。

dirty uploadもID Vec / HashSetへ戻しません。全stateがdirtyなら1個の正確なdense copyです。疎な変更は従来の連続rangeを保ち、fragmented bulkでrange数が128を越える場合は最初と最後のdirty IDの間を1 spanでuploadします。この場合だけunchanged gapsも含みますが、最大16 MB / 1Mで、数十万のallocation / queue writesを避けます。1 pieceの通常更新は引き続き16 bytesです。

## Performance invariants

| 経路 | CPU / upload |
| --- | --- |
| idle / selection保持 | O(1)。cached countsとArc identityだけを確認。state / mask upload 0 bytes |
| unfocused idle | local owner countのO(1)確認。owner maskを毎frame走査しない |
| pointer update | O(1)、piece position変更なし、command / state / membership / selected upload 0 |
| rectangle preview | CPU readbackなし。GPUのみ |
| point | 最大1 ID、4-byte readback。clipped ROIはcopyなし |
| rectangle final | O(N/32) readback / decode、authority再検証はO(selected)。ID Vecなし |
| rollback snapshot | O(1) Arc clone、mutation時だけ最大125 KBのcopy |
| drag start | member検証O(selected + N/32)、相対Zのsortはworst-case O(selected log selected)。command 1個 |
| drag release | O(selected)、command 1個、per-member完了eventなし |
| dirty upload準備 | bounded range allocations。dense commitはcopy 1個、通常の単一変更は16 bytes |

## 1Mのメモリ

decimal bytes。Arc header、Vec / enum metadata、O(players) owner counts、Bevy、texture、driver residencyは表の対象外です。OS RSS / peak resident memoryは未測定です。

| 領域 | bytes | lifetime / 備考 |
| --- | ---: | --- |
| CPU dense state | 16,000,000 | 正本、16 / piece |
| GPU dense state | 16,000,000 | 形式を変更していない |
| CPU selected mask | 125,000 | Arc words |
| previous selected | 0 | 削除 |
| original / rollback mask | 0 additional while shared; ≤125,000 on COW | pointer down snapshot |
| CPU requested drag membership | ≤125,000 | 全選択で全件有効ならselected allocationを共有 |
| CPU accepted membership | 0 additional if unchanged; ≤125,000 if filtered | authorityが拒否memberを除いた場合だけ別mask |
| CPU dirty mask | 125,000 | HashSet / sorted ID Vecなし |
| dense owner IDs + occupancy | 8,125,000 | 8,000,000 + 125,000、first holdで確保、release後も再利用 |
| GPU selected mask | 125,000 | 新規buffer |
| GPU preview mask | 125,000 | 既存buffer |
| GPU drag membership | 125,000 | 既存buffer |
| GPU selectable mask | 125,000 | 既存buffer |
| rectangle result + staging | 262,144 / slot | 各131,072へ丸め、最大3 slots = 786,432 |
| actual rectangle readback bytes | 125,000 | staging capacityと区別 |
| committed selection ID Vec | 0 | runtimeから削除 |
| persistent drag ID Vec | 0 | gestureから削除 |
| Grab Z-sort temporary IDs | 約4,000,000（Vec capacityによって約4.2 MB） | 4 / accepted member、authority内で破棄 |
| full dirty upload copy | 16,000,000 | main / renderがArcで共有、2 framesがoverlapすれば2 copies |
| raw result / decode temporary | 約250,000 + final mask125,000 | byte Vecとword Vecの移行中。ID単位のallocationはない |

all-validの全選択drag中はselected / requested / accepted / extractedが同じ125 KB allocationを共有します。dense state、owners、occupancy、dirty、selectedの論理CPU合計は24,375,000 bytesです。bulk start / releaseのpending full uploadを含めると40,375,000 bytes。Z sort、rollback COW、filtered membership、authority変更による表示maskのCOW（最大125 KB）、複数readbackやpipeline overlapは別途上記の領域を加算します。これは全processのpeak値ではありません。

## Multiplayer / migration

両bulk commandは`ClientCommandEnvelope`のreliable Control streamに属し、best-effort Move streamに載せると拒否します。session、epoch、authenticated player、duplicate sequenceの規則は変えません。serialized maskは31,250 wordsまでにdecodeを制限し、寸法・paddingも検証します。1Mのword payloadはbinary u32換算125 KBで、framing / codec overheadは別です。transport、Steam、compression、wire version negotiationは今回実装していません。

ReleaseGroupのdeltaはauthorityに保持しているpositionに対する最終移動です。group pointer updatesはlocal presentationのみです。将来のtransport adapterも同じgroupの途中にper-piece absolute Moveを送らず、reliable grabとfinal releaseを使う契約です。通信中のgroup presentation同期は今後の作業です。

snapshot schema 2とdense snapshot形式は変更していません。selection / original / preview / drag / ownerはsnapshotへ追加していません。captureのactive drag拒否、restoreのholds / selection reset、GPU epoch更新を維持し、既存migration testsを実行しています。disconnectの既存release_player_holdsは互換性のためID Vecを返しますが、通常dragの開始 / release / idle経路は使いません。

## Benchmark環境と手順

Windows、AMD Ryzen 9 9950X、Rust 1.97.0、NVIDIA GeForce RTX 5090、Vulkan、NVIDIA driver 610.88。release / locked。CPUは1k / 10k / 100k / 1Mを各5 samples、pointerは各sample 100,000 updates。CPU state commitのreleaseは全member non-snap、snap判定自体は全件実行します。snapshotのArc cloneは10,000回の平均、copy比較は125 KB words copy + count、HashSet比較はclone + immediate dropです。

1M rectangle benchmarkは2048² offscreen、4096²のpuzzle定義と1×1のopaque white texture、assembled state、5 samplesです。全1M hit、readback125 KB、highlightだけ125 KB、dense state upload0、drag pointer各updateの3種uploadが0、bulk start / release各16 MBをassertしています。GPU rectangle timeはROI + rectangle timestamp、CPU finalizationはreceive / decode、selection commit、highlight preparationの和です。channel待ち、GPU latency / map待ち、Bevy frame全体を含みません。grab / release frame値はCPU upload準備とGPU completion waitも含むfixture wall timeです。

既存24-row renderer benchmarkは1024² offscreen、実4096² RGBA8 texture、8 warmup + 30 frame平均、near / medium / entire × opaque / translucentを4サイズで維持しています。基準commitのsource copyもworktreeのtarget内で実行して比較しました。baselineで再使用されたproject release artifactsは破棄し、変更版を再compileして、追加testが実際に実行されたことを確認しています。

```sh
cargo fmt --check
cargo check --locked
cargo clippy --workspace --locked --all-targets --all-features -- -D warnings
cargo test --locked
cargo test --locked --all-features
cargo build --locked
cargo test -p puzzella-game --release --locked gpu_ -- --ignored --nocapture --test-threads=1
cargo test -p puzzella-game --release --locked million_selection_cpu_benchmark -- --ignored --nocapture --test-threads=1
cargo test -p puzzella-game --release --locked multi_drag_cpu_benchmark -- --ignored --nocapture --test-threads=1
```

benchmarkはworkspaceのtargetにmillion-selection-cpu.csv、million-selection-gpu.csv、procedural-benchmark.csvを出力します。GPU CSVにはadapter、backend、driverを含めます。既存renderer CSVの列は維持しています。CPU例は測定stageを分離しており、GPU frame / ordinary window FPSと混同しません。

## Results

CPU 5 samplesの中央値。stageの中央値の和と、totalの中央値は区別しています。

| 1M CPU stage | 時間 |
| --- | ---: |
| select-all fill | 22.300 µs |
| final bitset receive / decode | 56.000 µs |
| selection authority revalidation / commit | 0.6146 ms |
| highlight upload準備（state bytes = 0） | 6.400 µs |
| total CPU finalization | 0.6774 ms |
| shared selection snapshot clone + drop | 7.520 ns |
| 125 KB deep copy + count | 52.600 µs |
| old HashSet clone + drop comparison | 1.3876 ms |
| pointer down / rollback snapshot | 0.900 µs |
| drag membership + 1 command生成 | 0.6475 ms |
| Grab ownership + relative Z | 5.0481 ms |
| total drag start（upload準備除外） | 5.6836 ms |
| Grab dirty upload準備（16 MB） | 1.8308 ms |
| drag pointer update | 11.515 ns |
| release command生成 | 1.000 µs |
| delta commit + ownership release + snap判定 | 16.4342 ms |
| total release（upload準備除外） | 16.4357 ms |
| release dirty upload準備（16 MB / 1 range） | 2.2281 ms |

ownership、Z、snapはauthorityの同一loop / sort処理であるため、上記のcombined stageとして測定しています。1MのGrab / Release command数は各1、temporary ID VecはauthorityのZ sortのみです。CPU pointerの4サイズ間の中央値は以下です。

| selected | pointer ns / update |
| ---: | ---: |
| 1,000 | 11.507 |
| 10,000 | 11.517 |
| 100,000 | 11.472 |
| 1,000,000 | 11.515 |

実GPU fixtureの1M rectangleは、GPU ROI + rectangle 0.516864 ms、125,000-byte readback。CPU receive 25.400 µs、selection commit 0.9070 ms、highlight準備 6.600 µs、total CPU finalization 0.9390 ms。highlight uploadは125,000 bytes、state uploadは0です。fixture内のGrab authorityは4.5206 ms、release authorityは19.2126 ms。headless CPU値とは別sample / cache条件です。

既存rendererの1M比較（各row 30-frame平均）は以下です。drawにmaterialな悪化は観測せず、opaque full drawはほぼ同値です。cull / sortの増減とwall timeの変動もそのまま残しています。single paired runの比較であり、統計的な無退行保証や他GPUへの外挿はしません。SDF coverage、GPU sort順序、visible counts、bind group reuse、drag / preview / readbackのcorrectness testsはすべて成功しています。

| 1M view | frame before → after ms | cull before → after ms | draw before → after ms | sort before → after ms |
| --- | --- | --- | --- | --- |
| near | 1.1879 → 1.2762 | 0.0122 → 0.0124 | 0.0483 → 0.0413 | 0.0000 → 0.0000 |
| medium | 1.1674 → 1.2670 | 0.0122 → 0.0127 | 0.0529 → 0.0561 | 0.0000 → 0.0000 |
| entire | 1.9475 → 2.0158 | 0.0269 → 0.0276 | 0.4965 → 0.4898 | 0.0000 → 0.0000 |
| near_translucent | 1.6737 → 1.3079 | 0.0151 → 0.0151 | 0.0494 → 0.0429 | 0.1313 → 0.1307 |
| medium_translucent | 1.8018 → 1.5540 | 0.0153 → 0.0212 | 0.0527 → 0.0538 | 0.1229 → 0.1378 |
| entire_translucent | 2.4526 → 2.0112 | 0.0167 → 0.0163 | 0.5000 → 0.4992 | 0.2402 → 0.2687 |

記録: [CPU 20 samples](../benchmarks/million-selection-cpu.csv)、[1M rectangle GPU 5 samples](../benchmarks/million-selection-rtx5090.csv)、[current renderer 24 rows](../benchmarks/million-selection-renderer-rtx5090.csv)、[base renderer 24 rows](../benchmarks/million-selection-baseline-rtx5090.csv)、[environment](../benchmarks/million-selection-environment.json)。

最終validation: fmt、check --locked、all-target / all-feature clippy（warnings denied）、通常test 86件、all-feature test 86件、build、実GPU 7 tests、CPU million-selection benchmark、既存multi-drag pointer benchmarkがすべて成功しました。all-featureのTracy / Windows symbol初期化は既存のSymInitialize code 87を出力しましたが、test / commandは成功しています。


## Correctness / 制限

point / Ctrl add-remove、additive / empty rectangle、stale responses、preview / final ordering、rollback、1M全選択とdeselectを検証しています。single / multiple / 1M drag、finite pointer、focus loss / pause、exactly-once release、placed / mixed ownership、snap threshold / mixed snap、progress / completion、bounded serialized mask、invalid IDs / dimensions、migration restoreを検証しています。拒否されたGrab、scalar / bulk release後の別playerによる再Grab、古いHELD flagでもowner tableが有効な場合の拒否も検証します。fragmented dirty uploadは500,000 sparse IDsから1 spanへ縮約し、次のsingle editが16 bytesへ戻ることをassertします。

rendererのprocedural SDF、visibility compute、indirect draw、radix sort、preview passは維持しています。中級GPUは現在用意できないため未測定です。RTX 5090の数字を中級GPU性能へ外挿しません。将来の別GPUでは上記同じrelease commandsを実行し、adapter / backend / driver、CSV、window / resolutionとtexture条件を保存して比較してください。GPU testsはtimestamp対応adapterが必要です。

remaining costはexplicit final selectionのauthority再検証、Z sort、dense owner allocation、release時のsnapとstate copy / uploadです。fragmented uploadはallocation budgetと引き換えにunchanged gapsの転送を許容します。全normal window操作、異OS / GPU、RSS peakは未検証です。chunk spatial index、LOD、custom allocator、unsafe SIMD、vendor固有renderer変更は導入していません。
