//! Immutable table roots and path-copy index maintenance.

use crate::{
    error::{Error, Result},
    key::IndexKey,
    metrics::Metrics,
    schema::{IndexKind, TableSchema},
};
use hashbrown::{DefaultHashBuilder, HashMap as MutableHashMap};
use im::{OrdMap, OrdSet};
use std::{
    hash::{BuildHasher, Hash},
    sync::Arc,
};

const DIRECTORY_SHARDS: usize = 64;

#[derive(Debug)]
struct ChunkMap<K, V> {
    shards: Vec<Arc<MutableHashMap<K, V>>>,
    hash_builder: DefaultHashBuilder,
}

impl<K, V> Clone for ChunkMap<K, V> {
    fn clone(&self) -> Self {
        Self {
            shards: self.shards.clone(),
            hash_builder: self.hash_builder.clone(),
        }
    }
}

impl<K: Eq + Hash, V> ChunkMap<K, V> {
    fn new() -> Self {
        let hash_builder = DefaultHashBuilder::default();
        Self {
            shards: (0..DIRECTORY_SHARDS)
                .map(|_| Arc::new(MutableHashMap::with_hasher(hash_builder.clone())))
                .collect(),
            hash_builder,
        }
    }

    fn hash(&self, key: &K) -> u64 {
        self.hash_builder.hash_one(key)
    }

    fn get(&self, key: &K) -> Option<&V> {
        let hash = self.hash(key);
        self.shards[Self::shard_for_hash(hash)]
            .raw_entry()
            .from_hash(hash, |candidate| candidate == key)
            .map(|(_, value)| value)
    }

    fn contains_key(&self, key: &K) -> bool {
        self.get(key).is_some()
    }

    fn shard_for_hash(hash: u64) -> usize {
        usize::from(hash.to_le_bytes()[0]) % DIRECTORY_SHARDS
    }
}

impl<K: Clone + Eq + Hash, V: Clone> ChunkMap<K, V> {
    fn insert(&mut self, key: K, value: V) {
        let shard = Self::shard_for_hash(self.hash(&key));
        // 公開済みrootと共有するshardだけを複製し、以前のsnapshotの内容を保つ。
        Arc::make_mut(&mut self.shards[shard]).insert(key, value);
    }

    fn remove(&mut self, key: &K) {
        let shard = Self::shard_for_hash(self.hash(key));
        Arc::make_mut(&mut self.shards[shard]).remove(key);
    }
}

/// A generation-bearing logical row identifier.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct RowId {
    pub(crate) slot: u64,
    pub(crate) generation: u32,
}

#[derive(Clone, Debug)]
pub(crate) struct IndexRoot {
    pub(crate) kind: IndexKind,
    pub(crate) entries: OrdMap<IndexKey, OrdSet<RowId>>,
}

#[derive(Clone, Debug)]
struct PrimaryEntry<T> {
    row_id: RowId,
    record: Arc<T>,
}

impl IndexRoot {
    fn empty(kind: IndexKind) -> Self {
        Self {
            kind,
            entries: OrdMap::new(),
        }
    }

    fn remove(&mut self, key: &IndexKey, row_id: RowId) {
        let Some(mut rows) = self.entries.get(key).cloned() else {
            return;
        };
        rows.remove(&row_id);
        if rows.is_empty() {
            self.entries.remove(key);
        } else {
            self.entries.insert(key.clone(), rows);
        }
    }

    fn insert<T: TableSchema>(
        &mut self,
        index_id: usize,
        key: IndexKey,
        row_id: RowId,
    ) -> Result<()> {
        let mut rows = self.entries.get(&key).cloned().unwrap_or_default();
        if self.kind.is_unique() && rows.iter().any(|existing| *existing != row_id) {
            let definition = &T::metadata().indexes[index_id];
            return Err(Error::UniqueIndexViolation {
                table: T::metadata().name,
                index: definition.name,
            });
        }
        rows.insert(row_id);
        self.entries.insert(key, rows);
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) struct TableRoot<T: TableSchema> {
    primary: ChunkMap<T::PrimaryKey, PrimaryEntry<T>>,
    pub(crate) rows: OrdMap<RowId, Arc<T>>,
    versions: ChunkMap<T::PrimaryKey, u64>,
    pub(crate) indexes: Vec<IndexRoot>,
    pub(crate) next_slot: u64,
    pub(crate) tombstones: u64,
}

impl<T: TableSchema> Clone for TableRoot<T> {
    fn clone(&self) -> Self {
        Self {
            primary: self.primary.clone(),
            rows: self.rows.clone(),
            versions: self.versions.clone(),
            indexes: self.indexes.clone(),
            next_slot: self.next_slot,
            tombstones: self.tombstones,
        }
    }
}

impl<T: TableSchema> TableRoot<T> {
    pub(crate) fn empty() -> Self {
        Self {
            primary: ChunkMap::new(),
            rows: OrdMap::new(),
            versions: ChunkMap::new(),
            indexes: T::metadata()
                .indexes
                .iter()
                .map(|definition| IndexRoot::empty(definition.kind))
                .collect(),
            next_slot: 0,
            tombstones: 0,
        }
    }

    pub(crate) fn get(&self, key: &T::PrimaryKey) -> Option<Arc<T>> {
        self.primary.get(key).map(|entry| Arc::clone(&entry.record))
    }

    pub(crate) fn version(&self, key: &T::PrimaryKey) -> u64 {
        self.versions.get(key).copied().unwrap_or(0)
    }
}

#[derive(Debug)]
pub(crate) struct PendingEntry<T: TableSchema> {
    key: T::PrimaryKey,
    original_version: u64,
    original_exists: bool,
    value: Option<Arc<T>>,
}

#[derive(Debug)]
pub(crate) struct PendingTable<T: TableSchema> {
    table_id: usize,
    base_commit: u64,
    entries: Vec<PendingEntry<T>>,
    positions: MutableHashMap<T::PrimaryKey, usize>,
}

impl<T: TableSchema> PendingTable<T> {
    pub(crate) fn new(table_id: usize, base_commit: u64) -> Self {
        Self {
            table_id,
            base_commit,
            entries: Vec::new(),
            positions: MutableHashMap::new(),
        }
    }

    pub(crate) const fn table_id(&self) -> usize {
        self.table_id
    }

    fn effective_value(&self, base: &TableRoot<T>, key: &T::PrimaryKey) -> Option<Arc<T>> {
        self.positions
            .get(key)
            .map_or_else(|| base.get(key), |index| self.entries[*index].value.clone())
    }

    fn set_value(&mut self, base: &TableRoot<T>, key: T::PrimaryKey, value: Option<Arc<T>>) {
        if let Some(index) = self.positions.get(&key).copied() {
            self.entries[index].value = value;
            return;
        }
        let index = self.entries.len();
        // 同一キーへの再書き込みでも最初のversionを保持し、開始後の競合を検出する。
        self.entries.push(PendingEntry {
            original_version: base.version(&key),
            original_exists: base.primary.contains_key(&key),
            key: key.clone(),
            value,
        });
        self.positions.insert(key, index);
    }

    pub(crate) fn insert(&mut self, base: &TableRoot<T>, value: T) -> Result<()> {
        let key = value.primary_key();
        if self.effective_value(base, &key).is_some() {
            return Err(Error::DuplicatePrimaryKey {
                table: T::metadata().name,
            });
        }
        self.set_value(base, key, Some(Arc::new(value)));
        Ok(())
    }

    pub(crate) fn upsert(&mut self, base: &TableRoot<T>, value: T) {
        let key = value.primary_key();
        self.set_value(base, key, Some(Arc::new(value)));
    }

    pub(crate) fn update<F>(
        &mut self,
        base: &TableRoot<T>,
        key: &T::PrimaryKey,
        update: F,
    ) -> Result<()>
    where
        F: FnOnce(&mut T),
    {
        let Some(current) = self.effective_value(base, key) else {
            return Err(Error::NotFound {
                table: T::metadata().name,
            });
        };
        let mut changed = (*current).clone();
        update(&mut changed);
        if changed.primary_key() != *key {
            return Err(Error::PrimaryKeyChanged {
                table: T::metadata().name,
            });
        }
        self.set_value(base, key.clone(), Some(Arc::new(changed)));
        Ok(())
    }

    pub(crate) fn delete(&mut self, base: &TableRoot<T>, key: &T::PrimaryKey) -> Result<()> {
        if self.effective_value(base, key).is_none() {
            return Err(Error::NotFound {
                table: T::metadata().name,
            });
        }
        self.set_value(base, key.clone(), None);
        Ok(())
    }

    pub(crate) fn validate(&self, current: &TableRoot<T>, current_commit: u64) -> Result<()> {
        for entry in &self.entries {
            if current.version(&entry.key) != entry.original_version {
                return Err(Error::WriteConflict {
                    table: T::metadata().name,
                    base_commit: self.base_commit,
                    current_commit,
                });
            }
        }
        Ok(())
    }

    pub(crate) fn apply(
        &self,
        current: &TableRoot<T>,
        commit: u64,
        metrics: &Metrics,
    ) -> Result<TableRoot<T>> {
        let mut next = current.clone();

        // First remove obsolete index entries for every row. This makes unique-key
        // swaps inside one transaction legal and keeps validation atomic.
        for entry in &self.entries {
            let old = current.get(&entry.key);
            let Some(old_record) = old else { continue };
            let row_id = current
                .primary
                .get(&entry.key)
                .expect("visible primary entry")
                .row_id;
            let old_keys = old_record.index_keys();
            let new_keys = entry.value.as_ref().map(|record| record.index_keys());
            for (index_id, old_key) in old_keys.iter().enumerate() {
                let changed = new_keys
                    .as_ref()
                    .is_none_or(|keys| keys.get(index_id) != Some(old_key));
                if changed {
                    next.indexes[index_id].remove(old_key, row_id);
                    metrics.changed_index();
                }
            }
        }

        for entry in &self.entries {
            let existing_row_id = current.primary.get(&entry.key).map(|entry| entry.row_id);
            match &entry.value {
                Some(record) => {
                    let row_id = existing_row_id.unwrap_or_else(|| {
                        let id = RowId {
                            slot: next.next_slot,
                            generation: 0,
                        };
                        next.next_slot += 1;
                        id
                    });
                    let new_keys = record.index_keys();
                    let old_keys = current.get(&entry.key).map(|old| old.index_keys());
                    for (index_id, new_key) in new_keys.into_iter().enumerate() {
                        let changed = old_keys
                            .as_ref()
                            .is_none_or(|keys| keys.get(index_id) != Some(&new_key));
                        if changed {
                            if let Err(error) =
                                next.indexes[index_id].insert::<T>(index_id, new_key, row_id)
                            {
                                metrics.unique_violation();
                                return Err(error);
                            }
                            metrics.changed_index();
                        }
                    }
                    next.primary.insert(
                        entry.key.clone(),
                        PrimaryEntry {
                            row_id,
                            record: Arc::clone(record),
                        },
                    );
                    next.rows.insert(row_id, Arc::clone(record));
                    if !entry.original_exists && entry.original_version != 0 {
                        next.tombstones = next.tombstones.saturating_sub(1);
                        metrics.tombstone_removed();
                    }
                }
                None => {
                    if let Some(row_id) = existing_row_id {
                        next.primary.remove(&entry.key);
                        next.rows.remove(&row_id);
                        next.tombstones += 1;
                        metrics.tombstone_added();
                    }
                }
            }
            // 削除後もversionを残し、古いsnapshotによる再挿入を競合として扱う。
            next.versions.insert(entry.key.clone(), commit);
        }
        Ok(next)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
