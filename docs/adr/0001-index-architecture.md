# ADR 0001: Index architecture

- Status: Accepted for 0.1 alpha
- Date: 2026-08-13

## Context

更新ごとの全Table cloneを避けつつ、primary point lookupとordered range scanを両立する必要がある。Phase 0では次を比較した。

| Candidate | Read | Update copy | Range | Tail/complexity |
|---|---:|---:|---:|---|
| A: path-copy persistent tree | O(log n) | touched path | natural | proofしやすいがpoint readに比較回数 |
| B: immutable base + delta | base + deltas | small delta | merge iterator | compaction tailとunbounded delta管理が必要 |
| C: index別分離 | exactはhash、orderedはtree | chunk/path | natural | 実装が2系統だが目的に適合 |

初期prototypeの`im::HashMap -> RowId -> im::OrdMap` primary pathは100K rowsで637.68ns/op、`HashMap` 29.74ns/opとなり不採用とした。Candidate Cのchunked hash directoryへrecord pointerを直接格納すると、同じ環境の再測定でlive 100.38ns/op、retained snapshot 64.15ns/opまで改善した。まだ1.50倍KPIは未達であり、値は[BENCHMARK_REPORT.md](../../BENCHMARK_REPORT.md)に固定する。

## Decision

- primary: 64-way chunked `hashbrown::HashMap`。root cloneはshard `Arc`だけをcloneし、write batch内で触れたshardを一度だけCoWする。
- unique/exact/ordered/composite: `im::OrdMap<IndexKey, OrdSet<RowId>>`。安定したRowId順、range scan、path-copyを優先する。
- record directory: `im::OrdMap<RowId, Arc<T>>`。index iteratorとstable full scanに用いる。
- primary hot pathはrecord directoryを経由しない。
- old/new generated index keyを比較し、変化したindex pathだけremove/insertする。

## Consequences

全Table cloneは禁止され、更新copyはprimary/version directoryの該当shardとindex pathへ限定される。一方、1 commitごとに64 shard `Arc`とDB table-root vectorの浅いcloneがあり、write-heavy workloadでは今後のprofile対象となる。delta compactionは現行方式では不要だが、tombstone version metadataは明示的compaction未実装のため増加し得る。

