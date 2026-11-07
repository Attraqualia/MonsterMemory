# Safety policy and invariants

MonsterMemory workspaceは`unsafe_code = "forbid"`で構築する。独自のunsafe block、raw pointer、`transmute`、メモリレイアウト永続化はない。

## Invariants

1. `DatabaseRoot`はpublish後immutableである。
2. すべてのTable rootは同じ`DatabaseRoot.sequence`でatomicに公開される。
3. primary lookupはcommit gateを取得しない。snapshot/queryのlease登録・解放とmetrics取得は、別のmetrics用`Mutex`を使用する。
4. candidate rootはversion/unique/validation成功後にのみpublishされる。
5. `RecordRef`はrecord version自体を`Arc`で保持する。
6. `SnapshotLease`はrootを`Arc`で保持し、そのrootから到達するrecord/index nodeはdropされない。
7. checkpoint/WALのcontainer decoderはencoded payload lengthを上限検査してからbufferを確保し、checksum後にdeserializeする。Serdeがdecode中に確保する追加メモリは、この上限の対象外である。低水準の`decode_table`単体はchecksum・size limitを検査しない。

## 利用側のデータモデル

engineは公開済みrecordを直接変更せず、更新用に`Clone`した値を別versionとしてpublishする。ただし、record型が共有の内部可変状態を持つ場合、その状態まで凍結・deep copyはしない。snapshotの値を維持するには、利用側でも公開済みrecordや共有先を変更せず、primary/index keyの値を安定させる必要がある。`Clone`で共有する`Arc<Mutex<_>>`などを更新closureから変更すると、過去versionにも変更が見える可能性がある。

## 永続化の境界

CRC32は偶発的な破損検出であり、改ざんの認証ではない。schema fingerprintも衝突のない型証明ではなく、生成databaseのfingerprintはrecord内部のfield/index定義を網羅しない。永続化対象の型を変更した場合、fingerprint一致だけで互換性を判断しない。

checkpointの置換、WALのappend、WALの削除はそれぞれ別のfilesystem操作である。generated facadeは同じcoreのcommit gateでこれらを調整するが、別processや同じpathを開いた別database instanceとの排他は提供しない。低水準APIの直接利用時も、呼び出し側で書き込みを調整する。`Durability::Async`と`Flush`は現在同じ処理で、checkpointはどちらでも`atomic-write-file`のcommitがtemporary fileを同期する。親directory metadataの明示的な同期は本crateでは行わず、電源断時の挙動はfilesystem/deviceにも依存する。

## 依存関係と検証

不完全なWAL末尾は復旧時に読み飛ばすだけで、append前には修復されない。異常終了後に復旧したinstanceでは、同じpathへのcheckpointを成功させてからwriteを再開する。残った不完全なframeの後ろへのappendは、その後のcommitを復旧できなくする場合がある。

依存crate内部のunsafeは`unsafe_code = "forbid"`の対象外であり、workspaceのsafe Rust方針だけでは依存実装の安全性を証明しない。採用理由と代替を[DEPENDENCIES.md](DEPENDENCIES.md)へ記録する。Miriを含む検証は、実行したtoolchain・command・結果をその検証記録で確認する。本方針は、未実行の検査や第三者によるauditの完了を表明するものではない。

