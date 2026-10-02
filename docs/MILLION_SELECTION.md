# Million-piece selection / bulk interaction

2026-10-02追記: 本書の測定・schema 2・piece単位のrelease記述はbulk-selection導入時の記録です。永続連結後の現在のauthority、schema 3、追加8 MB、性能比較と維持／変更したinvariantは[CONNECTED_SNAPPING.md](CONNECTED_SNAPPING.md)を参照してください。idle / pointer O(1)、GPU point 4-byte / rectangle bitset、各transition 1 command、dense dirty uploadは維持し、final selection / Grab / Releaseへcomponent expansionと隣接snap探索を追加しました。

2026-10-02。branch `perf/million-piece-selection`、性能比較の基準 `014cab45fa61be5535cf857d62cabbe48204aeb8`。専用worktreeは `C:\Users\bebe\.codex\worktrees\million-selection\puzzella`。refreshしたorigin/masterから開始し、元workspaceの未コミット変更には触れていません。selection整合性・dirty mask再利用の追加修正は、文書をdocs/へ整理したmasterの`ca56bf8`に追従しています。載せ直し前後で検証済みコードとbenchmark CSVに差分がないことを確認しました。

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

`SelectionPayload::Point(Option<PieceId>)`と`Rectangle(PieceBitSet)`を分離しました。pointは4 bytes、final rectangleは4 × ceil(N / 32) bytesを受信し、wordのままmaskに変換します。寸法・末尾paddingを検証し、clipped / empty ROIのcopyを伴わない空応答は空maskへnormalizeします。latest requestと一致しない応答は従来どおり破棄します。GPU応答後に変わったplaced / ownershipをCPU authorityで再検証してからmaskをcommitします。additive selectionはoriginal側も再検証してからunionし、取消時のrollbackも同じ検証を行います。

新しく選択できる条件`is_selectable`と、既存selectionを維持できる条件`is_valid_local_selection`を分けています。後者はENABLED、非PLACED、ownerがNoneまたはLOCAL_PLAYERであることを要求し、自分がdrag中の選択を維持します。scalar / bulkで他playerがGrabした時、placedになった時、`set_state`によるauthority更新時に変更IDだけをselectionから除外します。毎frameのretainは追加しません。共有snapshotがある場合の最初の変更だけ、最大125 KBのCOW copyを伴います。

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

dirty uploadもID Vec / HashSetへ戻しません。全stateがdirtyなら1個の正確なdense copyです。疎な変更は従来の連続rangeを保ち、fragmented bulkでrange数が128を越える場合は最初と最後のdirty IDの間を1 spanでuploadします。この場合だけunchanged gapsも含みますが、最大16 MB / 1Mで、数十万のallocation / queue writesを避けます。1 pieceの通常更新は引き続き16 bytesです。range作成後にdirty maskをin-place clearし、次frameで同じword allocationを再利用します。frameごとの125 KB allocationは不要ですが、dirty frameのword走査とclearは残ります。

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
| authority変更後のselection | 変更IDだけを検証・除外。local holdは維持、毎frame走査なし |
| dirty upload準備 | mask allocationを再利用、bounded range allocations。dense commitはcopy 1個、通常の単一変更は16 bytes |

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
| CPU dirty mask | 125,000 | HashSet / sorted ID Vecなし、clearしてallocationを再利用 |
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
| select-all fill | 22.800 µs |
| final bitset receive / decode | 59.000 µs |
| selection authority revalidation / commit | 0.6997 ms |
| highlight upload準備（state bytes = 0） | 12.400 µs |
| total CPU finalization | 0.7707 ms |
| shared selection snapshot clone + drop | 7.620 ns |
| 125 KB deep copy + count | 48.400 µs |
| old HashSet clone + drop comparison | 1.4681 ms |
| pointer down / rollback snapshot | 1.200 µs |
| drag membership + 1 command生成 | 0.6536 ms |
| Grab ownership + relative Z | 5.3492 ms |
| total drag start（upload準備除外） | 6.0163 ms |
| Grab dirty upload準備（16 MB） | 1.7821 ms |
| drag pointer update | 11.812 ns |
| release command生成 | 0.700 µs |
| delta commit + ownership release + snap判定 | 16.5711 ms |
| total release（upload準備除外） | 16.5716 ms |
| release dirty upload準備（16 MB / 1 range） | 2.4280 ms |

ownership、Z、snapはauthorityの同一loop / sort処理であるため、上記のcombined stageとして測定しています。1MのGrab / Release command数は各1、temporary ID VecはauthorityのZ sortのみです。CPU pointerの4サイズ間の中央値は以下です。

| selected | pointer ns / update |
| ---: | ---: |
| 1,000 | 11.627 |
| 10,000 | 11.586 |
| 100,000 | 11.571 |
| 1,000,000 | 11.812 |

実GPU fixtureの1M rectangleは、GPU ROI + rectangle 0.520448 ms、125,000-byte readback。CPU receive 30.800 µs、selection commit 1.0628 ms、highlight準備 8.900 µs、total CPU finalization 1.1025 ms。highlight uploadは125,000 bytes、state uploadは0です。fixture内のGrab authorityは5.7625 ms、release authorityは20.6162 ms。headless CPU値とは別sample / cache条件です。

既存rendererの1M比較（各row 30-frame平均）は以下です。drawにmaterialな悪化は観測せず、opaque full drawはほぼ同値です。cull / sortの増減とwall timeの変動もそのまま残しています。single paired runの比較であり、統計的な無退行保証や他GPUへの外挿はしません。SDF coverage、GPU sort順序、visible counts、bind group reuse、drag / preview / readbackのcorrectness testsはすべて成功しています。

| 1M view | frame before → after ms | cull before → after ms | draw before → after ms | sort before → after ms |
| --- | --- | --- | --- | --- |
| near | 1.1879 → 1.6384 | 0.0122 → 0.0133 | 0.0483 → 0.0422 | 0.0000 → 0.0000 |
| medium | 1.1674 → 1.5538 | 0.0122 → 0.0133 | 0.0529 → 0.0546 | 0.0000 → 0.0000 |
| entire | 1.9475 → 2.1959 | 0.0269 → 0.0278 | 0.4965 → 0.5020 | 0.0000 → 0.0000 |
| near_translucent | 1.6737 → 1.8175 | 0.0151 → 0.0156 | 0.0494 → 0.0420 | 0.1313 → 0.1324 |
| medium_translucent | 1.8018 → 1.7873 | 0.0153 → 0.0155 | 0.0527 → 0.0532 | 0.1229 → 0.1251 |
| entire_translucent | 2.4526 → 2.7705 | 0.0167 → 0.0183 | 0.5000 → 0.4985 | 0.2402 → 0.2500 |

記録: [CPU 20 samples](../benchmarks/million-selection-cpu.csv)、[1M rectangle GPU 5 samples](../benchmarks/million-selection-rtx5090.csv)、[current renderer 24 rows](../benchmarks/million-selection-renderer-rtx5090.csv)、[base renderer 24 rows](../benchmarks/million-selection-baseline-rtx5090.csv)、[environment](../benchmarks/million-selection-environment.json)。

最終validation: fmt、check --locked、all-target / all-feature clippy（warnings denied）、通常test 91件、all-feature test 91件、build、実GPU 7 tests、CPU million-selection benchmark、既存multi-drag pointer benchmarkがすべて成功しました。all-featureのTracy / Windows symbol初期化は既存のSymInitialize code 87を出力しましたが、test / commandは成功しています。


## Correctness / 制限

point / Ctrl add-remove、additive / empty rectangle、stale responses、preview / final ordering、rollback、1M全選択とdeselectを検証しています。single / multiple / 1M drag、finite pointer、focus loss / pause、exactly-once release、placed / mixed ownership、snap threshold / mixed snap、progress / completion、bounded serialized mask、invalid IDs / dimensions、migration restoreを検証しています。拒否されたGrab、scalar / bulk release後の別playerによる再Grab、古いHELD flagでもowner tableが有効な場合の拒否も検証します。追加のselection回帰テストでは、scalar / bulk remote Grab後の除外、local holdの維持、authorityによるplaced / disabledへの変更、additive / PendingPoint・BoxSelecting rollbackでの古いoriginalの再検証を確認しています。dirty maskは1Mのsingle / dense更新でallocationのidentityを維持し、clear後に古いdirty bitsをuploadしないことを確認します。fragmented dirty uploadは500,000 sparse IDsから1 spanへ縮約し、次のsingle editが16 bytesへ戻ることをassertします。

rendererのprocedural SDF、visibility compute、indirect draw、radix sort、preview passは維持しています。中級GPUは現在用意できないため未測定です。RTX 5090の数字を中級GPU性能へ外挿しません。将来の別GPUでは上記同じrelease commandsを実行し、adapter / backend / driver、CSV、window / resolutionとtexture条件を保存して比較してください。GPU testsはtimestamp対応adapterが必要です。

remaining costはexplicit final selectionのauthority再検証、Z sort、dense owner allocation、release時のsnapとstate copy / uploadです。fragmented uploadはallocation budgetと引き換えにunchanged gapsの転送を許容します。全normal window操作、異OS / GPU、RSS peakは未検証です。chunk spatial index、LOD、custom allocator、unsafe SIMD、vendor固有renderer変更は導入していません。


## Small-component Grabのauthority最適化（2026-10-02）

作業開始時に`origin/master`をfetchし、最新の`9a7ac9f5ab0c0b97435b46ceb58f16ddd5fc2605`をbaselineにしました。指定された`0f07acc`以降の実装を含みます。専用branchは`codex/small-component-grab`、worktreeは`C:\Users\bebe\.codex\worktrees\small-component-grab\puzzella`です。元checkoutと他worktreeの未コミット変更は取り込んでいません。

### Authority flow / component atomicity

```text
Grab(id) → minimum_member(id) → grab_roots([minimum], no mask)
  → complete component validation → accepted ID Vec → apply_grab

small GrabGroup(mask) → lazy stable-minimum root extraction
  → complete component validation → accepted IDs / optional shared mask → apply_grab

dense GrabGroup(mask) → existing canonical component expansion / validation
  → accepted shared mask → ID-ordered Vec → apply_grab
```

scalarは入力mask、accepted mask、root dedup用maskを生成しません。大componentでもscalarはID単位のoccupancy / dirty更新を使い、一時N-bit maskを作りません。Z用Vecは受理したmember数Kに比例します。requested数R <= ceil(N/32)のGroupはroot fast pathを使い、singletonは直接処理します。連結componentはmask内にminimumがあればそのIDだけをemitします。minimumがmaskにない部分componentだけ`PieceScratchSet`で重複排除します。少数rootは128 IDsまでstackへ置き、129個目でdenseへ昇格します。root Vecは作らずiteratorで処理します。dense Groupは既存の`canonical_members` / `selectable_members`を維持し、DSU rootをscratchで重複排除してexpand / component検証を行います。この分岐でもcomponent atomicityは同じです。

`is_selectable`を全memberに適用してからacceptします。PLACED、disabled、HELD mirror、owner occupancyのいずれかがgrab不可ならcomponent全体をrejectし、別componentは独立してacceptします。現在player自身の既存holdも再Grabではrejectするため、duplicate commandでowner countを増やしません。dense owner IDs + occupancyとplayerごとのcountを維持し、per-component owner mapは追加していません。

受理した全componentのIDを一緒に`(z_order, PieceId)`でsortし、連続したfront Zを割り当てます。HashMap iterationには依存しません。MAX_Zのrare compactionは従来どおりsort後に実行します。dense Groupでは既存のaccepted maskからID順にVecを作り、従来のbulk pathの検証量・sort入力順・mask sharingを維持します。

### Local drag / selection / dirty同期

通常のlocal Groupは`Arc::ptr_eq(drag.members, requested.words())`で対応gestureを判定します。対応する場合だけrequested maskをcloneし、component expansion / rejectionを反映してaccepted ownershipに一致させます。全memberが有効なfull maskはArcを共有したままです。変更が必要な場合だけ最大125 KB / 1MのCOW copyを行い、全rejectではdragをresetします。scalar authority / compatibility pathはlocal pointer gestureを開始しないためdrag maskを生成しません。

remote Grabでは受理したcomponentの全memberをlocal selected maskとdrag表示maskから除外します。local Grabではselectionを維持します。small更新はoccupancy / dirty bitsをID単位で変更し、dense Groupは既に必要なaccepted maskをword単位でunionします。切替目安はK > ceil(N/32)です。const genericでID / word更新のloopをcompile時に分け、dense loopの各memberがsmall操作向けの分岐を払わないようにしています。通常のsingleton Grabは正確な16-byte uploadを維持し、dense / fragmented upload strategyは変更していません。MAX_Z compactionでは従来どおり全未配置stateをdirtyにする例外があります。

Release resolver、fixed translation、board priority、rounded fixed-offset closure、`rounded_offsets`、snapshot schema 3、renderer、command wire representationは変更していません。

### Complexity / memory

| 経路 | 計算量・allocation |
| --- | --- |
| scalar、owner領域再利用、ordinary Z | component lookup O(log k)、validation / hold O(k)、Z sort worst-case O(k log k)。temporary membership mask 0 bytes |
| Group | 入力mask走査 O(ceil(N/32) + R)、root lookup最大O(R log k_max)、touched component検証 O(K)、Z sort O(K log K)。dense出力を使う場合は追加bitmap走査 |
| small partial-root dedup | 最大128 IDsのinline set、heap 0。少数setの線形検索はbounded。129個目の昇格はO(N/32)のzero initializationを伴う |
| local drag同期 | all-valid full membershipはArc clone。expansion / rejectionはO(K)、最初のCOWはO(N/32) |
| pointer | O(1)。command、state / selection / membership uploadは0 |
| first hold / MAX_Z | owner初回確保O(N)、Z compaction O(N log N)の既存slow paths |

Rはrequested set bits数、Kはtouched componentのmember数、kはscalar component sizeです。owner再利用・共有state解放後のsmall scalar authorityから不要なN依存を除去しています。初期uploadがdense stateを共有している間の例外的な早期変更では、既存の16 MB / 1M COWが残ります。

恒久メモリの追加は0 bytesです。16-byte state、8-byte / piece connectivity、owner IDs + occupancy 8,125,000 bytes / 1Mを維持します。1M上の1 / 8 / 32 memberのscalar planにはmaskがなく、各Z Vec capacityが128 bytes以下であることをassertします。partial Groupの同条件ではroot scratch heap 0 bytesをassertします。既存owners / occupancy / dirty / dense stateのallocation identityも通常scalar apply前後で保持します。baselineのscalar入力maskは125,000-byte payload / 1Mで、連結expansion時は追加COWもありました。OS RSS / peak allocationは測定していません。

### 同じharnessによるbefore / after

Windows 11 / Ryzen 9 9950X / Rust 1.97.0 / release。全harness sourceのSHA-256を一致させ、baseline archived sourceと改善worktreeを別target directoryでbuildしました。最終CPU測定はvalidation buildとGPU testsの終了後に順番に実行しています。一時的な負荷の揺れがあったため、baseline→改善版、改善版→baseline、baseline→改善版の3組を集計しました。CSVのrun_passで各組を識別できます。詳しい環境・hash・test結果は[environment JSON](../benchmarks/small-grab-environment.json)にあります。

small Grabは各条件10 samples × 3組 = 30 samples、4 puzzle sizes × 3 component sizes × 3 commands × 2 owner-storage条件で各実装2160 samples。componentをN/2付近に作り、partial maskは中央の1member、full maskは全memberを指定します。timed regionは`apply_command`だけです。puzzle初期化、union、selection、mask作成、fixture生成、owner再利用のwarmupは測定外です。毎sampleで全component ownership、正確なdirty count、drag mask不要をassertしています。以下はowner領域再利用時の中央値（µs）。

| N | k | scalar before | scalar after | partial Group before | partial Group after | full Group before | full Group after |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1000 | 1 | 0.30 | 0.10 | 0.20 | 0.10 | 0.20 | 0.10 |
| 1000 | 8 | 0.50 | 0.30 | 0.40 | 0.30 | 0.30 | 0.30 |
| 1000 | 32 | 0.95 | 1.00 | 0.80 | 1.05 | 0.70 | 1.10 |
| 10000 | 1 | 1.15 | 0.10 | 1.00 | 0.30 | 1.00 | 0.30 |
| 10000 | 8 | 1.60 | 0.40 | 1.40 | 0.70 | 1.30 | 0.60 |
| 10000 | 32 | 2.10 | 1.40 | 2.00 | 1.60 | 1.90 | 1.50 |
| 100000 | 1 | 8.40 | 0.30 | 8.05 | 2.20 | 8.00 | 1.80 |
| 100000 | 8 | 9.60 | 0.70 | 8.85 | 2.80 | 8.40 | 2.25 |
| 100000 | 32 | 9.95 | 4.10 | 10.80 | 4.80 | 9.20 | 3.90 |
| 1000000 | 1 | 123.55 | 1.00 | 78.65 | 16.05 | 77.65 | 16.20 |
| 1000000 | 8 | 143.65 | 2.70 | 99.40 | 18.50 | 79.90 | 17.75 |
| 1000000 | 32 | 147.85 | 7.75 | 101.40 | 24.30 | 82.20 | 22.90 |

[small Grab baseline CSV](../benchmarks/small-grab-baseline-small-grab-cpu.csv)、[改善後CSV](../benchmarks/small-grab-optimized-small-grab-cpu.csv)に全samplesを保存しています。Groupのpartial / full差が小さいのは、少数componentの処理よりdense input maskの走査が支配するためです。scalarとGroupの残るN依存は区別が必要です。

owner初回確保を含む1Mの中央値（µs）。これはtemporary scratchではなく、既存8.125 MBの恒久owner storageの確保・初期化コストです。

| k | scalar before | scalar after | partial Group before | partial Group after | full Group before | full Group after |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 963.30 | 813.75 | 917.55 | 852.25 | 917.65 | 812.85 |
| 8 | 1017.75 | 1199.20 | 996.50 | 930.95 | 938.05 | 852.65 |
| 32 | 1077.15 | 853.50 | 969.10 | 883.90 | 942.45 | 847.95 |

既存million-selection harnessの1M全選択Grabは6.3767 → 6.0805 ms（約4.6%減、5 samples × 3組の中央値、初回owner確保込み）です。[baseline](../benchmarks/small-grab-baseline-million-selection-cpu.csv)、[改善後](../benchmarks/small-grab-optimized-million-selection-cpu.csv)を参照してください。

既存connected-snapping harnessの1M中央値（ms）。Grabではselection / dragが準備済み、Releaseは同じresolverとassertを使用します。

| scenario | Grab before | Grab after | Release before | Release after |
| --- | ---: | ---: | ---: | ---: |
| disconnected | 6.1309 | 6.0516 | 48.5882 | 48.1370 |
| single_snap | 0.9374 | 0.8597 | 0.0436 | 0.0518 |
| closure | 0.9349 | 0.8797 | 86.0325 | 86.0825 |
| board | 6.2430 | 5.8582 | 136.3770 | 134.7949 |
| connected | 9.9368 | 9.6138 | 24.4555 | 24.6337 |

[connected baseline](../benchmarks/small-grab-baseline-connected-snapping-cpu.csv)、[改善後](../benchmarks/small-grab-optimized-connected-snapping-cpu.csv)。Release / snappingのproduction codeに変更はありません。small Releaseも各条件10 samples × 3組で同じfixtureを再実行しました。1M中央値（µs）は以下です。

| k | scalar before | scalar after | partial Group before | partial Group after |
| ---: | ---: | ---: | ---: | ---: |
| 1 | 7.15 | 6.80 | 12.55 | 13.20 |
| 8 | 9.45 | 9.30 | 15.75 | 15.95 |
| 32 | 13.50 | 11.85 | 18.35 | 18.85 |

[small Release baseline](../benchmarks/small-grab-baseline-small-release-cpu.csv)、[改善後](../benchmarks/small-grab-optimized-small-release-cpu.csv)。短時間測定はcache状態や実行順によって変動します。performance数値はframe latencyではありません。

既存multi-drag benchmark、各100,000 pointer updatesの1回あたり平均を3組で測った中央値（ns）。

| selected | before | after |
| ---: | ---: | ---: |
| 1000 | 11.684 | 11.978 |
| 10000 | 11.751 | 12.086 |
| 100000 | 11.688 | 12.041 |
| 1000000 | 11.721 | 12.154 |

[pointer baseline](../benchmarks/small-grab-baseline-multi-drag.csv)、[改善後](../benchmarks/small-grab-optimized-multi-drag.csv)。million-selectionとconnected-snapping harnessでもpointer中のmembership Arc不変、state allocation不変、dirtyなしをassertします。GPU testsはpointer frameのstate / selection / membership upload 0も検証します。

### Correctness / validation

追加testsは1M scalar singleton / 8 / 32 memberのmask不要、scalar connected、partial / duplicate references、stable minimumとDSU rootの相違、mixed / stale ownership、placed、disabled、remote selection全component除外、local selection維持、複数componentのcross-component Z / PieceId tie / MAX_Z、duplicate owner count、partial dragのaccept/reject同期、full drag Arc sharing、root extraction helperのfull mask scratch heap不要、129以上partial rootsのdense昇格、大component scalar、singleton 16-byte uploadを検証します。既存のmillion component round tripとRelease / snapping / snapshotのtestsも通っています。

必須6チェックはすべてexit 0。通常testは161 passed / 0 failed / 13 ignored、all-featuresは161 passed / 0 failed / 13 ignored。release gameは110 passed / 0 failed / 13 ignored。CPU benchmark5件はbaseline / 改善版とも成功。RTX 5090 / Vulkan / NVIDIA 610.88で既存GPU filterの7件と画像upload 1件も成功しました。

```sh
cargo fmt --check
cargo check --locked
cargo clippy --workspace --locked --all-targets --all-features -- -D warnings
cargo test --locked
cargo test --locked --all-features
cargo build --locked
cargo test -p puzzella-game --release --locked small_component_grab_cpu_benchmark -- --ignored --nocapture --test-threads=1
cargo test -p puzzella-game --release --locked small_component_release_cpu_benchmark -- --ignored --nocapture --test-threads=1
cargo test -p puzzella-game --release --locked connected_snapping_cpu_benchmark -- --ignored --nocapture --test-threads=1
cargo test -p puzzella-game --release --locked million_selection_cpu_benchmark -- --ignored --nocapture --test-threads=1
cargo test -p puzzella-game --release --locked multi_drag_cpu_benchmark -- --ignored --nocapture --test-threads=1
cargo test -p puzzella-game --release --locked gpu_ -- --ignored --nocapture --test-threads=1
cargo test -p puzzella-game --release --locked render_only_image_upload_keeps_metadata_and_pixel_values -- --ignored --nocapture --test-threads=1
```

今回の実行は`CARGO_TARGET_DIR` / `--target-dir`で比較用targetを分離しています。GPU coverage、Z、alpha、visibility、picking、drag、connected outline、preview/readback、画像pixelとuploadの既存assertを維持しました。通常windowでの全手動操作や異OS / GPUの確認は含みません。

### 主な変更ファイル / 残存N依存 / future work

| file | 変更 |
| --- | --- |
| `game/src/resources/pieces.rs` | small root extraction / validation、dense canonical path維持、optional maskのGrabPlan、small ID / dense word更新、drag同期 |
| `game/src/resources/pieces/connected_tests.rs` | Grab correctness / memory / upload tests |
| `game/src/interaction_bench.rs` | scalar / partial / full × reused / firstのsmall Grab benchmark |
| `benchmarks/small-grab-*.csv`, `small-grab-environment.json` | 同一harnessの全samples、環境・validation |
| `docs/MILLION_SELECTION.md` | 本追記、complexity、before / after、制限 |

残るN依存はGroup commandのdense input bitmap iteration、129以上partial rootsのscratch昇格、dense Groupのaccepted mask走査 / COW、selection canonical output mask、local drag membership / remote selection COW、dense owner初回確保、initial state共有中のCOW、dirty upload準備時のword走査 / clear、MAX_Z compactionです。Z sortはaccepted member数Kに依存し、全選択時はK=Nになります。普通のsmall scalar Grabのauthority pathとbulk / presentationの経路は分けて評価してください。

`GrabGroup` / `ReleaseGroup`のwire `PieceBitSet`は1M puzzleで125,000-byte payloadのままです。将来の候補は`Component(PieceId)`、Sparse IDs、Dense BitSetのadaptive encodingです。transport、packet format、snapshot schemaは今回変更していません。
