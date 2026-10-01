# 可視数に応じた透明画像の GPU radix sort

2026-10-01。透明画像のfull-capacity bitonic sortを、可視IDだけを対象とした安定radix sortへ変更しました。旧経路の1M時210 dispatchから、radix本体9 dispatch、準備と可視ID圧縮を含めて11 dispatchへ削減しています。capacity全体へのsentinel初期化も削除しました。

## 処理と描画順

`DrawArgs.instance_count`がGPU上のvisible countです。CPUへreadbackせず、`prepare_sort`が`ceil(visible_count / 256)`をindirect dispatch bufferへ書き込みます。各radix passはhistogram・bucket prefix scan・stable scatterの3 dispatchで、shiftは0 / 8 / 16です。histogramとscatterは可視workgroupだけを実行し、prefix scanもその可視workgroupのcountsだけを走査します。0件ならradixのindirect workgroup数は全て0です。

旧bitonicの比較キーは`(placed ? 0 : z_order + 1, PieceId)`でした。24bit上限の`MAX_Z = 2²⁴ - 2`により`z_order + 1`も24bitに収まります。placedを最初に描き、loose pieceを小さいZから大きいZへ描くalpha blend順序を維持しています。

atomic appendだけでは同じZの入力ID順が不定になります。透明経路ではcull workgroup内でID順に圧縮し、workgroup countsのexclusive scanと再圧縮で全可視IDをID順に並べます。radixのscatterはbucket membership bitsetの先行lane数から局所順位を求め、group offsetsとbucket offsetsを加えます。3 pass全てが安定なので同じZは旧経路と同じID昇順になります。subgroup機能や浮動小数点キーに依存しません。

主な実装は`game/src/render/visibility.wgsl`、`radix_sort.wgsl`、`mod.rs`です。3 passのping-pongをscratch → visible → scratch → visibleとし、main drawとpickingが使うvisible bufferへ結果を戻します。描画・選択のshape、alpha discard、depth設定は従来と同じです。不透明経路はatomic appendを使い、radix scratchを確保しません。透明から不透明へ切り替えた場合のscratchはセッション終了まで保持します。

## 計算量とメモリ

Nは全ピース数、Vは可視数、Gは`ceil(N / 256)`です。cullingは従来どおりO(N)。順序維持のための可視ID圧縮はO(G + V)、radixは固定256 buckets、3 passでO(V)。1MでVが1000ならhistogram / scatterは各pass4 workgroupsになり、capacity全体のcompare / swapを実行しません。prefix scanは可視数が非ゼロなら256 workgroupsです。

main / picking visible ID bufferはpower-of-two paddingをやめ、各`4N` bytesへ縮小しました。透明画像で追加するscratchは以下です。GPU state（16 bytes/piece）とCPU正本の形式は変更していません。

| 追加領域 | bytes | N = 1,000,000 |
| --- | ---: | ---: |
| ping-pong ID scratch | 4N | 4,000,000 |
| cull group counts / offsets | 4(G + 1) | 15,632 |
| radix group histogram / offsets + bucket totals | 4 × 256 × (G + 1) | 4,001,792 |
| indirect dispatch commands | 24 | 24 |
| 合計 | 4N + 1028(G + 1) + 24 | 8,017,448（約7.65 MiB） |

small uniform・bind group・driver追加領域は含みません。メモリ確保はNに依存しますが、毎frameのradix走査範囲はVに依存します。

## 実GPU検証

Windows / RTX 5090 / Vulkanで、100万件の同一セッションに対してVを0、1、255、256、257、1000、65,537、1,000,000、1000、0と変え、GPU結果をCPUの比較キーsortと完全一致させています。疎なID、全件表示、partial workgroup、prefix scanが256 groupsを超える場合、同じZ、placed、8bit / 16bit境界、24bit上限を含みます。indirect argsもreadbackし、workgroup数がVに応じて変わることをassertしています。

既存のalpha blend画素、透明穴越しのpoint / rectangle選択、drag delta / membership、GPU preview、tabだけの可視性、camera / viewport、opaque raster coverageも検証します。

```sh
cargo test -p puzzella-game --release --locked gpu_ -- --ignored --nocapture --test-threads=1
cargo test -p puzzella-game --release --locked procedural_gpu_benchmark -- --ignored --nocapture
```

benchmarkは1k / 10k / 100k / 1Mで、不透明・透明それぞれのnear / medium / entireを計測します。GPU時間はtimestampで30 frames平均、frame時間はoffscreen描画とGPU同期waitを含むfixture時間です。sort時間は準備・圧縮を含み、cull時間は別に記録します。実測24行は[CSV](../benchmarks/radix-sort-rtx5090.csv)に記録しています。1024² offscreen、4096² RGBA8画像、v5 shape、同じdense stateを正解位置へ並べた状態です。

| N | 透明view | visible | histogram / scatterのworkgroups / pass | sort GPU ms | cull GPU ms |
| ---: | --- | ---: | ---: | ---: | ---: |
| 1,000,000 | near | 1,156 | 5 | 0.1266 | 0.0158 |
| 1,000,000 | medium | 10,404 | 41 | 0.1290 | 0.0159 |
| 1,000,000 | entire | 1,000,000 | 3,907 | 0.2612 | 0.0185 |

旧bitonicの全体表示sort 2.5750 msは[PROCEDURAL_RENDERER.md](PROCEDURAL_RENDERER.md)のv3移行時の記録です。今回のv5・別runの値と区別して残しています。near / mediumの小さな差は固定dispatch / scanコストと測定変動を含み、Vに対する厳密な時間比例を意味しません。

共有workspaceで別の初期化・メモリ変更も進行中だったため、基準`4fec6dc5fa3a960d69ad7ffe866fcacc3014f07c`に今回のrender差分を重ねた検証用コピーで、fmt、check、全target / 全feature clippy（warnings denied）、通常test、全feature test、build、GPU 4 testsとbenchmarkを実行し、全て成功しました。CPUの初期化計測はこのコピーの旧Vec経路です。全feature testのWindows profiling初期化は既知の`SymInitialize FAILED code 87`を出力しましたが、testとコマンドは成功しました。画素検証fixtureではBevyのoutput pipelineを含むstartupの非同期compile待ちをなくすため、pipeline compilationを同期化しています。runtimeのcompile設定は変更していません。

最後に共有workspaceでもcheck、fmt、全target / 全feature clippyとGPU 4 testsを再実行して成功し、並行したdense state変更との組み合わせでも描画・選択・sort順序を確認しました。

commit時に更新されたmasterのbind group cacheとも統合しました。radixのping-pong用2 groupsとdispatch準備用groupをsession内で再利用し、初めて透明へ切り替える際は新しいcull counts bufferのIDもcompute cache keyへ含めます。cacheの再利用・epoch切替・不透明→透明切替はGPU testで確認します。CSVはcache統合前の測定値です。
