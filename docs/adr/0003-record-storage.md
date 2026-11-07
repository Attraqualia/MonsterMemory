# ADR 0003: Record storage and reclamation

- Status: Accepted for 0.1 alpha
- Date: 2026-08-13

各record versionは`Arc<T>`でimmutableに保持する。logical `RowId { slot, generation }`とrecord directoryを分離し、updateは同じRowIdに新versionを割り当て、deleteはprimary/indexから除去する。削除keyのversionはwrite conflict検出用tombstoneとして保持する。

回収は独自epochやraw pointerを使わず、published/retained rootからの`Arc`到達可能性に委ねる。これによりold snapshotがdropされる前にrecord/index nodeを解放しない。現行alphaはslot再利用を行わず、ABAを構造的に避ける。generationは将来の安全なslot reuse用である。

長寿命snapshotは`active_snapshots`と`oldest_snapshot_age`で観測できる。retained bytesの正確な集計とbackground compactionは未実装である。

