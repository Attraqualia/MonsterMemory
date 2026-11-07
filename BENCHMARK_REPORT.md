# Benchmark report

## Environment

- Date: 2026-08-13
- CPU: Intel Core i7-7700 @ 3.60GHz, 4 cores / 8 logical processors
- RAM: 17,053,212,672 bytes (~15.9 GiB)
- OS: Microsoft Windows 10 Pro 10.0.19045 x86_64
- rustc: 1.94.1 (e408947bf 2026-03-25), MSVC target
- crate: 0.1.0-alpha.1
- profile: Cargo `bench`, `lto = "thin"`, `codegen-units = 1`, system allocator
- dataset: deterministic 100,000 `u64 -> u64` records; 2,000,000 wrapping pseudorandom key reads
- command: `cargo bench -p monstermemory --bench read_path`
- source: `crates/monstermemory/benches/read_path.rs`; raw CSV is printed by the command

## Latest result

Second warm run after the chunked hash hot-path change:

| Implementation | ns/op | vs HashMap |
|---|---:|---:|
| `HashMap<u64,u64>` | 30.76 | 1.00x |
| `RwLock<HashMap<u64,u64>>` | 33.56 | 1.09x |
| MonsterMemory live `get` | 100.38 | 3.26x |
| MonsterMemory retained-snapshot `get` | 64.15 | 2.09x |

Timing is a simple whole-loop wall-clock measurement, not a statistical confidence interval. Shared-machine frequency scaling and Windows scheduling add noise; the preceding run was 25.43 / 32.43 / 116.37 / 67.64 ns respectively. Both complete runs are reported instead of selecting only the favorable values.

## Architecture spike result

The rejected initial primary route used persistent hash lookup to RowId followed by persistent ordered-directory lookup. It measured 29.74ns HashMap, 48.69ns RwLock baseline, 637.68ns MonsterMemory live, and 548.94ns snapshot. A chunked hash primary directory holding the record Arc directly reduced the MonsterMemory results substantially without whole-table cloning.

## KPI status and bottleneck

- Primary point read target ≤1.50x HashMap: **not met** (3.26x live, 2.09x snapshot on latest warm run).
- Warm read allocation: **met in test**; `allocation-counter` asserts 0 allocations over 1,000 live and snapshot reads after warm-up.
- Concurrent read throughput and p99 under write: correctness stress exists, but calibrated throughput/percentile matrix is **not yet measured**.
- 1M/10M dataset, memory/index footprint, copied bytes/commit, compaction pause, hardware counters: **not yet measured**.

The remaining point-read overhead is atomic root acquisition/downcast, chunked directory lookup, and two reference-count operations (root/record depending on API). Next work is profiler-guided root projection/cache design that preserves database-wide snapshots, followed by Criterion-style distributions and allocation instrumentation in benchmark mode. No unsafe optimization will be considered before a safe prototype demonstrates the gain.

## Reproduction variants

```text
MONSTERMEMORY_BENCH_ROWS=1000000 MONSTERMEMORY_BENCH_ITERS=5000000 cargo bench -p monstermemory --bench read_path
```

PowerShell:

```text
$env:MONSTERMEMORY_BENCH_ROWS=1000000
$env:MONSTERMEMORY_BENCH_ITERS=5000000
cargo bench -p monstermemory --bench read_path
```
