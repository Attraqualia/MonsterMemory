# MonsterMemory 開発者マニュアル

An Attraqualia project, developed by halnos.

Version 0.1.0-alpha.1。本文はJavaScriptなしでも読めます。JavaScript有効時は検索とcopy buttonが使えます。保守担当はhalnosです。

> Alpha: APIとcheckpoint formatは変更され得ます。未検証のProduction Ready、lock-free writer、Serializableを表明しません。

## Overview

MonsterMemoryはRust structとderive attributesからtyped CRUD、index query、database-wide snapshot、multi-table write transactionを生成するembedded in-memory databaseです。通常利用で必要な概念はDatabase、Table、Read/query、Write、Snapshot/transactionの5つです。

## 5-minute Quick Start

次はCIでコンパイルを確認する実ファイルです。

{{#include crates/monstermemory/examples/quickstart.rs}}

## Installation

このworkspaceのルートで`cargo run -p monstermemory --example quickstart`を実行します。別アプリケーションから使う場合はチェックアウトを`MonsterMemory`として隣に配置し、次のpath dependencyを設定します。crates.io公開は前提にしません。memory-onlyがdefaultで設定ファイルは不要です。

```toml
[dependencies]
monstermemory = { path = "../MonsterMemory/crates/monstermemory" }
```

## Schema Definition

`#[derive(Clone, Table)]`をrecordへ、`#[derive(Database)]`を`Table<T>` fieldだけを持つdatabase facadeへ付けます。generic/lifetime table、unknown attribute option、重複生成名はcompile errorです。

## Primary Key

各Tableは`#[primary_key]`をちょうど1つ持ちます。keyはClone + Debug + Eq + Hash + Send + Sync + 'staticが必要です。update closureがkeyを変更するとcommit前に`PrimaryKeyChanged`を返します。

## Secondary / Unique Index

`#[index]`は1 keyから複数rowへのexact query、`#[index(unique)]`は`Option<RecordRef<T>>`を返します。unique制約はcurrent rootに対してcommit gate内で再検証されます。non-unique結果はstable RowId順です。

## Range / Composite Index

`#[index(range)]`は`RangeBounds`を受け取るlazy forward iteratorを生成します。`#[composite_index(name = "country_age", fields(country, age), kind = "ordered")]`はlexicographic prefix + suffix range queryを生成します。

{{#include crates/monstermemory/examples/indexes.rs}}

## CRUD

`insert`、`update`、`delete`、`upsert`はsingle-operation auto commitです。`get`、`get_cloned`は読み取りでありcommitを発生させません。`get`の`RecordRef<T>`はrecord versionのArcを保持し、database更新後も保持できます。failed writeはpublished rootを変更しません。レコード内で共有する内部可変状態はengineが固定しないため、snapshotとして固定したい値はClone後の更新で旧versionに影響しない設計が必要です。

## Batch Write / Bulk Load

`insert_many`、`upsert_many`、`delete_many`は各callを1 transactionとしてatomic publishします。`Database::bulk_load`は全Table loaderを1 commitで公開します。現在は入力をstreamしながらpersistent structureへbatch適用し、parallel sort/buildは未実装です。

## Transactions

`write` closure内の複数Table操作はatomicです。closureを暗黙再実行しません。`retrying_write(RetryPolicy, ...)`だけは名前とdocumentどおり複数回実行し得るため、外部side effectを置かないでください。

{{#include crates/monstermemory/examples/transactions.rs}}

## Snapshot / MVCC Semantics

保証はSnapshot Isolationです。snapshotは全Tableで同じcommit sequenceを保持し、repeatable readを提供します。write transactionは開始rootを基準にし、同一keyのversion変更をfirst-committer-winsで拒否します。predicate read-setは追跡せずSerializableではありません。

{{#include crates/monstermemory/examples/snapshots.rs}}

## Concurrency Model

primary lookupはArcSwapからimmutable rootをloadし、writerのcommit mutexを取得しません。snapshot/query作成時は保持状態をmetricsへ登録するmutexを取得します。writerはtransaction-local write-setを準備後、commit gateでconflict/unique/validatorを検査し、DB rootを一度だけpublishします。永続化時はWALのエンコードとI/Oもgate内で行うため、writerの待ち時間はストレージ性能にも依存します。writer publicationはserializedです。

## Error Handling

public `Result<T>`はnon-generic `Error`を使います。`DuplicatePrimaryKey`、`UniqueIndexViolation`、`NotFound`、`WriteConflict`、`PrimaryKeyChanged`、`SchemaMismatch`、`CorruptData`、`Io`、`Configuration`があります。`is_retryable()`は現在`WriteConflict`だけtrueです。

## Validation

`#[database(validate = "validate_database")]`でcandidate snapshotを受けるcross-table validatorを設定できます。validator失敗時はpublishされません。row validation専用attributeとincremental foreign keyは今後の課題です。

## Persistence / Durability

`persistence` featureと`#[database(persistence)]`を指定し、recordにSerde Serialize/Deserializeをderiveすると`open`と`checkpoint`が生成されます。formatはmagic、u16 version、schema fingerprint、little-endian length、CRC32、postcard payloadです。`open`はcheckpointを検査後、存在するWALの最新の完全なimageを優先して復元し、そのインスタンスの以後のwriteをFsync付きWALへ記録します。`memory`は永続化hookを設定しません。同じpathを複数のdatabaseインスタンスやプロセスで共有しないでください。

低レベルAPIのDurabilityはAsync、Flush、Fsyncです。AsyncとFlushのWAL処理は現実装では同じであり、Asyncも同期I/Oです。checkpointのatomic置換は依存ライブラリ側でも同期を行います。完全な電源断耐性の保証は行いません。

不完全なWAL末尾は復旧時に読み飛ばすだけで、自動修復しません。異常終了後に`open`で復旧した場合は、同じpathへのcheckpointを成功させてから追加writeを行ってください。残った末尾の後ろへappendすると、その後のcommitを復旧できなくなる場合があります。

{{#include crates/monstermemory/examples/persistence.rs}}

## Schema Evolution

保存したfingerprintの不一致は`SchemaMismatch`です。ただしdatabaseのfingerprintはdatabase型名・table field名・record型の表記から算出し、recordの全field定義やSerde設定を含みません。同じrecord型名のままfieldを変更すると一致し得るため、一致だけでschema互換とは判断できません。schema変更時は互換性を利用側で確認してください。自動migrationはありません。

## Compaction / Memory Reclamation

old root/record/index nodeはArc到達可能性で安全に回収されます。snapshotを長く保持すると回収が遅れるためactive snapshot数とoldest ageをmetricsで確認できます。slot再利用、tombstone compaction、background compactionは未実装です。

## Performance Tuning

default設定で開始してください。single readの反復ではexplicit snapshotを再利用するとroot reloadを避けられます。大量初期化は`bulk_load`、複数変更はbatch/transactionを使います。memory limit builderは0を拒否するだけで使用量を制限しません。snapshot_warning_afterも自動通知を設定しません。

## Metrics / Tracing

`db.metrics()`はcommit sequence、active snapshots、oldest snapshot age、commits、conflicts、unique violations、changed index paths、tombstonesを返します。read hot pathにcounter/loggingは置きません。tracing adapterとretained bytesは未実装です。

## Testing Strategy

unit、integration、BTreeMap differential property test、concurrent readers/writers、first-committer-wins、trybuild UI、Loom縮約model、corrupt checkpointを検証します。seed失敗時はproptest regressionを保存できます。fuzz entryは`fuzz/fuzz_targets/checkpoint_decoder.rs`です。

## Architecture

Read path: ArcSwap root → chunked primary hash → Arc record。Index path: ArcSwap/snapshot root → persistent ordered index → RowId → persistent record directory。Write path: base root → typed write-set → commit gate → version/unique/validation → atomic root publish。

```text
READ:   ArcSwap root ──> TableRoot ──> primary shard ──> Arc<Record>
RANGE:  SnapshotLease ─> ordered index ─> RowId ───────> record directory
WRITE:  base root ─────> typed write-set ─> commit gate ─> candidate ─> publish
LIFE:   old root ──────> retained snapshot ─> drop ─────> Arc reclamation
CRASH:  commit ────────> WAL frame + CMIT ─> publish; open ─> replay latest frame
```

## Safety Invariants

published rootはimmutable、snapshotが到達可能なobjectは解放しない、candidateはvalidation完了前にpublishしない、persistence inputはlength/checksum検査後にdecodeする、workspace独自codeはunsafe禁止、が主要invariantです。

## Benchmark Methodology and Results

`cargo bench -p monstermemory --bench read_path`をrelease profile、100K rows、2M random lookupsで実行します。2026-08-13のWindows x86_64測定ではHashMap 30.76ns、RwLock HashMap 33.56ns、live get 100.38ns、snapshot get 64.15nsでした。1.50倍KPIは未達です。詳細はrepository rootの`BENCHMARK_REPORT.md`です。

## Migration Guide

0.1 alpha以前の公開versionはありません。将来のmigrationではgenerated method名、feature flags、checkpoint format version、schema fingerprintをsemver/compatibility review対象にします。

## Troubleshooting

missing primary errorは1 fieldへ`#[primary_key]`を追加します。index trait bound errorはfieldをIndexValue対応型へするか、明示的なcanonical encoding対応を待ちます。WriteConflictは新transactionでclosureを再構築します。SchemaMismatch時にfileを無理にdecodeしないでください。

## FAQ

SQL、network server、distributed replication、cross-process shared memory、full ORMはありますか: ありません。Readerはwait-freeですか: MonsterMemory自身として形式保証は表明せず、ArcSwapのupstream保証範囲に依存します。Serializableですか: いいえ、Snapshot Isolationです。

## Feature Flags

defaultはmemory-onlyです。`persistence`はportable checkpoint crate、Serde integration、generated open/checkpointを有効にします。experimental serializableやasync runtime featureはありません。

## MSRV / Platform Support

editionはRust 2024、MSRVは1.88、検証toolchainは1.94.1です。CI matrixはWindows/Linux/macOS current stableとMSRVを定義します。coreはTokioを必要としません。

## Security / Corrupt Input Handling

container readerはpayloadの符号化サイズを既定1GiB上限、checked length、CRC32で検査し、checkpointではexact file sizeも検査します。WALは不完全な最終frameを許容します。Serdeのdecode時に作るobjectのメモリ使用量まで制限するものではありません。CRC32は認証ではありません。脆弱性の報告手順はrepository rootのSECURITY.mdを参照してください。

## API Reference

`cargo doc -p monstermemory -p monstermemory-core -p monstermemory-macros -p monstermemory-persistence --features monstermemory/persistence --no-deps`でlocal API referenceを生成します。source-of-truthはcrate rustdocとcompileされるexamplesです。
