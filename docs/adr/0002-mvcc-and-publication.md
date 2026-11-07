# ADR 0002: MVCC and atomic publication

- Status: Accepted for 0.1 alpha
- Date: 2026-08-13

DB全Tableを含む`DatabaseRoot`を`ArcSwap`でatomic publishする。readerはcurrent rootまたはexplicit snapshot rootを読み、commit mutexを取得しない。writerは開始rootに対してtyped write-setを準備し、短いcommit gate内で各keyのversionをcurrent rootと比較する。異なれば`WriteConflict`、一致すればunique制約とvalidatorを含むcandidate rootを完成させ、一度だけpublishする。

保証はSnapshot Isolationである。repeatable read、database-wide同一sequence、lost update防止、atomic multi-table publicationを提供する。predicate read-setは追跡しないためSerializableとは表明しない。arbitrary closureを自動再実行せず、明示的な`retrying_write`だけが再実行する。

`Mutex` poisoning時はinner stateを回収する。panicしたwrite closureはcommitへ到達せず、未公開write-setと共にdropされる。

