# Packed-key update sorting experiment

Candidate `f4fb30dfaf9ff55f27f87b0376b2d7b910221cf3` is based on `33c491663d96c545e49e8af4cb6edc233132f0ce`. It changes only the update comparator from unpacked `Nibbles` to the original `B256` key. The candidate is experimental; these measurements qualify it for a real-block Derek comparison and do not establish a node-throughput improvement.

The input map has unique 32-byte keys. Every derived path is exactly 64 nibbles, so byte lexicographic order is identical to nibble lexicographic order. The sorted sequence, update grouping, and proof callback order are unchanged. No allocation or trie data structure is added. The existing 180 sparse tests passed with the benchmark compiler/features, and nightly formatting plus `git diff --check` passed.

## Measurement method

Both frozen executables use Rust 1.98.1, the `profiling` profile, `-C target-cpu=native`, `alloy-primitives/asm-keccak`, and `alloy-primitives/keccak-cache-global`. The harness SHA256 is `fede8b3ab85dc11baf03a586b385c02010680b387e9f7f4be28dd3f3bc8d84c2`. The hardware is an AMD EPYC 4585PX. Original matrices use one Rayon thread on CPU 1 or 15 Rayon threads on CPUs 1–15. Each has two ABBA rounds: four fresh processes per variant, 100 measured samples plus five warmups per case and process. `summary.csv` contains medians and inclusive quartiles of process medians; these are not confidence intervals.

The synthetic fixtures are storage tries with 1,000 or 32,768 Keccak-distributed keys and RLP U256 values. `update_root` times applying updates, hashing, and taking retained branch records, excluding database I/O. `root_dirty` times hashing updates already applied outside the timer; `root_cached` times the cached root return. Setup, cloning, input allocation, and reference-root verification are untimed. Every timed result is checked against Alloy HashBuilder. Identical repeated inputs warm the global Keccak cache. These standalone executables use the system allocator rather than the reth binary allocator. Excluded setup still influences cache and Rayon worker state.

## Original full matrix

All retained bulk update cases improve: 4.8–20.2% with one thread and 4.3–41.0% with 15 threads. The largest 15-thread case saves 3.87 ms. The table reports median time reduction, so positive values are faster.

| Keys | Changed | Retained update + root, 1 thread | Retained update + root, 15 threads |
| ---: | ---: | ---: | ---: |
| 1,000 | 1 | 4.60 → 4.56 µs (+0.76%) | 4.48 → 4.41 µs (+1.57%) |
| 1,000 | 100 | 109.70 → 104.44 µs (+4.80%) | 100.85 → 96.55 µs (+4.27%) |
| 1,000 | 1,000 | 437.82 → 363.29 µs (+17.02%) | 372.87 → 287.42 µs (+22.92%) |
| 32,768 | 1 | 7.08 → 7.15 µs (-1.05%) | 8.71 → 9.08 µs (-4.32%) |
| 32,768 | 3,276 | 3990.96 → 3690.44 µs (+7.53%) | 1297.90 → 999.30 µs (+23.01%) |
| 32,768 | 32,768 | 17812.63 → 14220.84 µs (+20.16%) | 9418.59 → 5553.39 µs (+41.04%) |

Most root-only controls are near flat, but the 15-thread retained 32,768-key single-change root median increased from 6.958 to 7.880 µs (+0.922 µs, 13.25% slower). Its corresponding update/root median increased from 8.707 to 9.083 µs (+0.376 µs, 4.32% slower). The retained cached-root median increased from 65 to 105 ns (+40 ns). These observations are retained rather than removed from the result.

## Control analysis

The changed comparator does not execute within either root-only timed region. A single-element update sort performs no comparisons. Inspection confirms the two binaries have identical `root` and benchmark function addresses, instruction counts, and normalized instructions; only RIP-relative data references and direct-call target relocations differ (`control-disassembly.json`). This rules out a changed root algorithm and changed layout of those two function bodies; it does not rule out effects from called code or data placement.

The original 15-thread single-change retained-root process medians vary from 6.682–8.325 µs in the baseline and 6.923–8.296 µs in the candidate. FoldHash uses per-map randomized seeds, so hash-map layout is not held identical across processes. Trie clones, setup execution, CPU migration within the affinity mask, and Rayon worker state also remain uncontrolled below the process level. These are plausible sources of the observed variation, not proven causal explanations. The focused repeat below tests whether the gaps recur; it does not identify their cause.

## Focused controls

The focused repeats use the same frozen binaries, 15 Rayon threads, and CPUs 1–15. Each filter has four ABBA rounds: eight fresh processes per variant and 200 measured samples per case. The full-matrix preceding timed cases are skipped; the harness still constructs all fixtures and computes reference roots outside the timed regions. Local build/test jobs and large artifact downloads were held until these repeats finished. No source or benchmark-harness edits were made.

| Focused control | Baseline median | Candidate median | Candidate change | Process-median IQR, baseline / candidate |
| --- | ---: | ---: | ---: | --- |
| `root_dirty/32768/1/retain` | 7,995.0 ns | 8,020.5 ns | +0.32% (+25.5 ns) | 7,646.5–8,342.8 / 7,649.8–8,097.8 ns |
| `update_root/32768/1/retain` | 9,708.5 ns | 9,597.5 ns | -1.14% (-111.0 ns) | 9,357.2–10,161.8 / 9,054.2–10,356.8 ns |
| `root_cached/32768/discard` | 90.0 ns | 90.5 ns | +0.56% (+0.5 ns) | 77.8–102.5 / 85.2–110.0 ns |
| `root_cached/32768/retain` | 85.5 ns | 90.0 ns | +5.26% (+4.5 ns) | 77.5–100.0 / 77.5–98.2 ns |

The earlier +13.25% single-change root gap and +40 ns cached-root gap did not reproduce. All focused control process-median quartiles overlap. The results support treating those controls as inconclusive small effects, not as established comparator regressions or established zero cost. Absolute medians also shifted between the full and focused workloads, reinforcing that setup and process conditions matter. Both datasets remain separate and preserved; they are not pooled or selectively substituted.

## Decision

The packed comparator is semantically equivalent, simple, and produces consistent, substantial bulk update-plus-root savings in both thread configurations. It qualifies for an unprofiled real-block Derek experiment. It is not yet a demonstrated overall payload-latency improvement, and the isolated experiment does not establish behavior when combined with the separate database candidates. Adoption should use paired Derek mean/tail latency and relevant phase data; these local measurements alone do not establish faster end-to-end execution.

## Artifacts and reproduction

- `metadata.json`: candidate source, compiler, features, validation, and executable hash.
- `threads-1/` and `threads-15/`: original full-matrix metadata, per-process CSVs, and summaries.
- `focused-single-15/` and `focused-cached-15/`: filtered four-round repeats, with raw process CSVs and metadata.
- `control-disassembly.json`: bounded comparison of root/benchmark function instructions and placement.
- `packed-sort.patch`: production-only comparator change.
- `state-root-packed-sort`: frozen candidate executable.
- Baseline executable: `/tmp/reth-mpt-bench/state-root-retained33-native-cache`, SHA256 `2445b0cdc518be7de6fa10ceb94816890b34f4491129917a5830d73e168294d8`. An archived identical copy is `../hash-mask-experiment/state-root-retained-33`.
- Baseline and candidate harness inputs and reference roots are identical.

```sh
python3 scripts/bench-trie.py \
  /tmp/reth-mpt-bench/state-root-retained33-native-cache \
  bench-work/trie-derek-20260904/packed-sort-experiment/state-root-packed-sort \
  --output /tmp/packed-sort-repeat --threads 15 --cpus 1-15 --samples 100 --rounds 2
```

Focused-repeat commands, using a new output directory for each invocation:

```sh
python3 scripts/bench-trie.py \
  /tmp/reth-mpt-bench/state-root-retained33-native-cache \
  bench-work/trie-derek-20260904/packed-sort-experiment/state-root-packed-sort \
  --output /tmp/packed-sort-single-repeat --threads 15 --cpus 1-15 \
  --samples 200 --rounds 4 --filter /32768/1/retain

python3 scripts/bench-trie.py \
  /tmp/reth-mpt-bench/state-root-retained33-native-cache \
  bench-work/trie-derek-20260904/packed-sort-experiment/state-root-packed-sort \
  --output /tmp/packed-sort-cached-repeat --threads 15 --cpus 1-15 \
  --samples 200 --rounds 4 --filter root_cached/32768
```
