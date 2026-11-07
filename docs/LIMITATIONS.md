# 既知の制約

MonsterMemory 0.1.0-alpha.1の制約です。APIとcheckpoint／WAL形式はalpha段階であり、変更される可能性があります。

## トランザクションとsnapshot

- 保証する分離レベルはSnapshot Isolationです。書き込むprimary keyのversionを検査し、競合時はfirst-committer-winsで拒否します。predicateの読み取り集合は追跡しません。
- writerの公開処理は直列化されます。永続化を有効にすると、WALのエンコードとI/Oもcommit gate内で行います。
- `retrying_write`はcommit競合時にclosureを再実行します。closure内の外部への副作用は、データベースのrollback対象ではありません。
- snapshotとlazy queryは旧rootを保持します。長時間の保持はrecordやindex nodeの回収を遅らせます。
- recordが共有する内部可変状態は固定されません。snapshotの値を維持するには、`Clone`後の更新が旧versionの共有先を変更しないデータモデルが必要です。

## メモリと監視

- builderの`memory_limit_bytes`は0を拒否しますが、使用量を制限しません。`snapshot_warning_after`による自動通知も未実装です。
- slot再利用、tombstone compaction、background compaction、保持バイト数の測定は未実装です。
- metricsは複数の観測値から構成されます。一部のカウンターは、後で検証に失敗する候補rootの処理でも増えるため、成功したcommitだけの集計にはなりません。

## 永続化

- 同じcheckpoint／WAL pathは、単一のdatabase instanceが排他的に管理してください。instance間・process間のfile lockはありません。
- 不完全なWAL末尾は復旧時に読み飛ばすだけで、自動修復しません。異常終了後に`open`で復旧した場合は、同じpathへのcheckpointを成功させてから追加writeを行ってください。
- database fingerprintはdatabase型名、table field名、record型の表記から算出し、recordの全field定義やSerde設定は含みません。fingerprintが一致しても、schema変更後のデータ互換性は利用側で確認する必要があります。自動migrationはありません。
- WALは各commitでdatabase全体のimageを記録します。operation差分のWALとgroup commitは未実装です。
- CRC32は偶発的な破損を検出するもので、改ざんの認証ではありません。encoded payloadのサイズ上限は、Serdeのdecode時に確保するobjectの総メモリ量を制限しません。
- `Durability::Async`と`Flush`のWAL処理は現実装では同じです。Asyncも同期I/Oを使用します。checkpointのatomic置換は依存ライブラリ側でも同期を行い、電源断時の結果はfilesystemやdeviceにも依存します。

## 性能と機能

100K件のデータで測定したprimary lookupは`HashMap`の約3.3倍の所要時間で、1.50倍以内という目標には達していません。測定条件、結果、未測定項目は[BENCHMARK_REPORT.md](../BENCHMARK_REPORT.md)を参照してください。

bulk loadはatomicな一括公開を提供しますが、parallel sort／index buildは未実装です。row validation専用attribute、foreign-key attribute、reverse iterator、tracing adapterも現在のAPIにはありません。

安全性の前提は[SAFETY.md](../SAFETY.md)、脆弱性の報告方法は[SECURITY.md](../SECURITY.md)を参照してください。
