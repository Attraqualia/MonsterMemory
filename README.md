# MonsterMemory

An Attraqualia project, developed by halnos.

**Rustの構造体からテーブルと索引を定義できる、型付きMVCCインメモリデータベース。**

> **Alpha** — APIとcheckpoint／WAL形式は変更される可能性があります。

## 導入・実行

アプリケーションのプロセス内にデータを保持するRustライブラリです。最小対応バージョンはRust 1.88、開発用ツールチェーンは`rust-toolchain.toml`で1.94.1に固定しています。OSに対応するRustのリンカーも必要です。

リポジトリを取得し、サンプルを実行します。

```sh
git clone https://github.com/Attraqualia/MonsterMemory.git
cd MonsterMemory
cargo run -p monstermemory --example quickstart
cargo run -p monstermemory --example indexes
cargo run -p monstermemory --example transactions
cargo run -p monstermemory --example snapshots
cargo run -p monstermemory --example persistence --features persistence
```

各サンプルは成功すると終了コード0で終了し、`assert`で結果を確認します。永続化サンプルはOSの一時フォルダにサンプル用checkpointを作成して削除します。

別のアプリケーションから使う場合は、チェックアウトを`MonsterMemory`という名前でアプリケーションの隣に配置し、次のpath依存を設定します。

```toml
[dependencies]
monstermemory = { path = "../MonsterMemory/crates/monstermemory" }
```

永続化を使う場合は上の依存宣言に`features = ["persistence"]`を追加し、レコード型にSerdeの`Serialize`と`Deserialize`を実装します。実例は[persistence.rs](crates/monstermemory/examples/persistence.rs)を参照してください。

## 最小例

```rust
use monstermemory::prelude::*;

#[derive(Clone, Debug, Table)]
#[table(name = "users")]
pub struct User {
    #[primary_key]
    pub id: u64,
    #[index(unique)]
    pub email: String,
    #[index]
    pub country: String,
    #[index(range)]
    pub age: u16,
}

#[derive(Database)]
pub struct AppDb {
    pub users: Table<User>,
}

fn main() -> monstermemory::Result<()> {
    let db = AppDb::memory();
    db.users.insert(User {
        id: 1,
        email: "alice@example.com".into(),
        country: "JP".into(),
        age: 28,
    })?;
    db.users.update(&1, |user| user.age += 1)?;
    assert_eq!(db.users.get(&1).map(|user| user.age), Some(29));
    Ok(())
}
```

完全な実行例は [crates/monstermemory/examples/quickstart.rs](crates/monstermemory/examples/quickstart.rs) です。

## 主な特徴

- database-wide immutable rootによるSnapshot Isolation
- primary lookupでwriterのcommit mutexを取得しない。warm primary readはテスト上0 allocation（snapshot／query作成時のmetrics登録にはmutexを使用）
- primary、unique、non-unique exact、range、ordered composite indexをderiveから自動保守
- single-operation auto commitとatomic multi-table transaction
- first-committer-wins write conflict、atomic unique validation、primary key変更検出
- persistent index nodeとchunked hash directoryによる差分copy
- `persistence` featureのchecksummed checkpoint、schema fingerprint、size limit
- unsafe codeなし（依存crateのunsafe利用は[DEPENDENCIES.md](DEPENDENCIES.md)に記録）

## Performance

測定条件と再現手順は [BENCHMARK_REPORT.md](BENCHMARK_REPORT.md) を参照してください。100K件のデータで測定したprimary lookupの所要時間は、`HashMap`に対して約3.3倍です。1.50倍以内という性能目標には達していません。

## Safety / Maturity

公開済みrootは不変で、snapshotが保持するrecordは`Arc`によって生存期間を維持します。core／facade／macro／persistenceは`unsafe`を禁止しています。詳細は [SAFETY.md](SAFETY.md) と [既知の制約](docs/LIMITATIONS.md) を参照してください。

利用にあたっては、次の制約があります。

- 保証はSnapshot Isolationです。predicateの読み取り集合は追跡しないため、Serializableではありません。
- writerの公開処理は直列化されます。長時間保持するsnapshotやqueryは旧rootの回収を遅らせます。
- `retrying_write`だけがcommit競合時にclosureを再実行します。closureに外部への副作用を置く場合は再実行を考慮してください。
- `memory_limit_bytes`は0の指定を拒否しますが、使用メモリ量を制限しません。`snapshot_warning_after`による自動通知も未実装です。
- 永続化はalpha形式です。CRC32は破損検出用であり認証ではなく、スキーマ変更の自動移行や複数プロセス間の排他制御は提供しません。
- database fingerprintはrecordの全field定義を含みません。schema互換性は利用側で確認してください。
- 不完全なWAL末尾は復旧時に読み飛ばすだけで修復しません。異常終了後は`open`の後、同じpathへのcheckpointを成功させてから追加writeを行ってください。

## Documentation

- [日本語開発者マニュアル](docs/manual/html/index.html)（`cargo xtask manual`で再生成）
- [Architecture Decision Records](docs/adr/0001-index-architecture.md)
- API reference: `cargo doc -p monstermemory -p monstermemory-core -p monstermemory-macros -p monstermemory-persistence --features monstermemory/persistence --no-deps --open`
- [開発・検証手順](CONTRIBUTING.md)、[脆弱性の報告](SECURITY.md)
- [既知の制約](docs/LIMITATIONS.md)

## Feature flags

- default: memory-only、追加機能なし
- `persistence`: `#[database(persistence)]`向けportable checkpoint API

## 開発構成

| クレート | 役割 |
| --- | --- |
| `monstermemory` | 利用者向けの入口、prelude、examples、統合テスト |
| `monstermemory-core` | MVCC、テーブル、クエリ、インデックス、metrics |
| `monstermemory-macros` | `Table`／`Database` deriveと型付きAPIの生成 |
| `monstermemory-persistence` | checkpointとWALのコンテナ、検査、I/O |
| `xtask` | 日本語HTMLマニュアルの生成と検査（配布用ライブラリではありません） |

保守担当はhalnosです。公開リポジトリは[Attraqualia/MonsterMemory](https://github.com/Attraqualia/MonsterMemory)です。

## License / Attribution

`MIT OR Apache-2.0`。ライセンス本文は[LICENSE-MIT](LICENSE-MIT)と[LICENSE-APACHE](LICENSE-APACHE)を参照してください。

MonsterMemoryは独立した実装であり、Cysharp公式製品、MasterMemory公式移植、API互換実装ではありません。read-heavy typed databaseという領域の先行例として、Cysharpの[MasterMemory](https://github.com/Cysharp/MasterMemory)を参考資料にしましたが、ソースコードはコピーしていません。通常の依存ライブラリについては[DEPENDENCIES.md](DEPENDENCIES.md)を参照してください。
