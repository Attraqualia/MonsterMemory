//! Typed table handles used by generated database facades.

#![allow(
    clippy::missing_errors_doc,
    reason = "operation-specific variants are documented on the public Error enum"
)]

use crate::{
    engine::{DatabaseCore, SnapshotLease, TransactionContext},
    error::Result,
    key::IndexKey,
    metrics::MetricsSnapshot,
    query::{AllQuery, IndexQuery, RecordRef},
    schema::TableSchema,
};
use std::{ops::Bound, sync::Arc};

/// A live typed table handle.
///
/// Created as a field of a database derived through `monstermemory`. Clones share
/// the same database. Each read independently observes a currently published root;
/// use the generated database snapshot for a consistent view across several reads.
///
/// Every write method on this type stages and commits one transaction. Returning
/// an error leaves the published database unchanged, and write conflicts are not
/// retried automatically. In addition to operation-specific errors, every commit
/// can return [`crate::Error::Configuration`] if its sequence is exhausted, an
/// error from a configured validator, or a persistence error when a hook is active.
/// User callback or iterator side effects are outside this atomicity guarantee.
#[allow(
    clippy::struct_field_names,
    reason = "table_id identifies a generated schema slot"
)]
pub struct Table<T: TableSchema> {
    core: Arc<DatabaseCore>,
    table_id: usize,
    marker: std::marker::PhantomData<fn() -> T>,
}

impl<T: TableSchema> Clone for Table<T> {
    fn clone(&self) -> Self {
        Self {
            core: Arc::clone(&self.core),
            table_id: self.table_id,
            marker: std::marker::PhantomData,
        }
    }
}

impl<T: TableSchema> std::fmt::Debug for Table<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Table")
            .field("name", &T::metadata().name)
            .field("table_id", &self.table_id)
            .finish_non_exhaustive()
    }
}

impl<T: TableSchema> Table<T> {
    /// Creates a generated table handle.
    #[doc(hidden)]
    #[must_use]
    pub fn __new(core: Arc<DatabaseCore>, table_id: usize) -> Self {
        Self {
            core,
            table_id,
            marker: std::marker::PhantomData,
        }
    }

    /// Returns the database core for generated facade code.
    #[doc(hidden)]
    #[must_use]
    pub fn __core(&self) -> Arc<DatabaseCore> {
        Arc::clone(&self.core)
    }

    /// Returns an immutable record handle for a primary key.
    ///
    /// Returns `None` if the key is absent from the observed root. The handle
    /// retains that row version after later commits without cloning `T`.
    #[must_use]
    pub fn get(&self, key: &T::PrimaryKey) -> Option<RecordRef<T>> {
        self.core
            .with_current(|root| root.table::<T>(self.table_id).get(key).map(RecordRef::new))
    }

    /// Returns a cloned record for callers that explicitly need ownership.
    ///
    /// Returns `None` for an absent key. Unlike [`Self::get`], this calls `T::clone`.
    #[must_use]
    pub fn get_cloned(&self, key: &T::PrimaryKey) -> Option<T> {
        self.get(key).map(|record| (*record).clone())
    }

    /// Number of live records in the current root.
    ///
    /// This value can change before a separate [`Self::get`] or [`Self::iter`] call.
    #[must_use]
    pub fn len(&self) -> usize {
        self.core
            .with_current(|root| root.table::<T>(self.table_id).rows.len())
    }

    /// Returns whether the current table is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Inserts a record as one auto-commit transaction.
    ///
    /// # Errors
    ///
    /// Returns [`crate::Error::DuplicatePrimaryKey`] if the primary key already
    /// exists, [`crate::Error::WriteConflict`] if it changes after staging, or
    /// [`crate::Error::UniqueIndexViolation`] if a secondary key would be duplicated.
    /// Other commit failures are described on [`Table`].
    pub fn insert(&self, value: T) -> Result<()> {
        let transaction = self.core.__transaction();
        WriteTable::<T>::__new(transaction.clone(), self.table_id).insert(value)?;
        self.core.__commit(transaction)?;
        Ok(())
    }

    /// Inserts all records atomically.
    ///
    /// No input row is published unless the entire operation succeeds. An empty
    /// input makes no commit. The iterator is consumed until completion or error.
    ///
    /// # Errors
    ///
    /// Returns [`crate::Error::DuplicatePrimaryKey`] for a key already present or
    /// repeated in the input, [`crate::Error::WriteConflict`] for a concurrent
    /// write to an input key, or [`crate::Error::UniqueIndexViolation`] for a
    /// duplicate secondary key. See [`Table`] for other commit failures.
    pub fn insert_many<I>(&self, values: I) -> Result<()>
    where
        I: IntoIterator<Item = T>,
    {
        let transaction = self.core.__transaction();
        let table = WriteTable::<T>::__new(transaction.clone(), self.table_id);
        table.insert_many(values)?;
        self.core.__commit(transaction)?;
        Ok(())
    }

    /// Inserts an iterable using the same atomic operation as [`Self::insert_many`].
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::insert_many`], with no partial publication.
    pub fn extend<I>(&self, values: I) -> Result<()>
    where
        I: IntoIterator<Item = T>,
    {
        self.insert_many(values)
    }

    /// Inserts or replaces a record as one auto-commit transaction.
    ///
    /// The record's primary key selects the row to replace. An absent key is
    /// inserted; a replacement preserves the row's logical iteration position.
    ///
    /// # Errors
    ///
    /// Returns [`crate::Error::WriteConflict`] for a concurrent write to the same
    /// primary key or [`crate::Error::UniqueIndexViolation`] for a conflicting
    /// secondary key. See [`Table`] for other commit failures.
    pub fn upsert(&self, value: T) -> Result<()> {
        let transaction = self.core.__transaction();
        WriteTable::<T>::__new(transaction.clone(), self.table_id).upsert(value);
        self.core.__commit(transaction)?;
        Ok(())
    }

    /// Inserts or replaces all records atomically.
    ///
    /// If input records share a primary key, the last one wins. Secondary-index
    /// uniqueness is checked against the final candidate. Empty input makes no
    /// commit; any failure prevents publication of every input row.
    ///
    /// # Errors
    ///
    /// Returns [`crate::Error::WriteConflict`] for a concurrent write to any input
    /// key or [`crate::Error::UniqueIndexViolation`] for duplicate final secondary
    /// keys. See [`Table`] for other commit failures.
    pub fn upsert_many<I>(&self, values: I) -> Result<()>
    where
        I: IntoIterator<Item = T>,
    {
        let transaction = self.core.__transaction();
        let table = WriteTable::<T>::__new(transaction.clone(), self.table_id);
        for value in values {
            table.upsert(value);
        }
        self.core.__commit(transaction)?;
        Ok(())
    }

    /// Updates one row using a transaction-local clone.
    ///
    /// Calls `update` once on a clone of the captured row. The closure must retain
    /// its primary key; on failure the changed clone is discarded. Previously
    /// returned [`RecordRef`] handles continue to see the original version.
    ///
    /// # Errors
    ///
    /// Returns [`crate::Error::NotFound`] if the key is absent,
    /// [`crate::Error::PrimaryKeyChanged`] if the closure changes the key,
    /// [`crate::Error::WriteConflict`] for a concurrent write to that key, or
    /// [`crate::Error::UniqueIndexViolation`] for a duplicate secondary key.
    /// See [`Table`] for other commit failures.
    pub fn update<F>(&self, key: &T::PrimaryKey, update: F) -> Result<()>
    where
        F: FnOnce(&mut T),
    {
        let transaction = self.core.__transaction();
        WriteTable::<T>::__new(transaction.clone(), self.table_id).update(key, update)?;
        self.core.__commit(transaction)?;
        Ok(())
    }

    /// Deletes one row as an auto-commit transaction.
    ///
    /// Previously returned row handles and snapshots retain their old versions.
    ///
    /// # Errors
    ///
    /// Returns [`crate::Error::NotFound`] if the key is absent or
    /// [`crate::Error::WriteConflict`] for a concurrent write to the key.
    /// See [`Table`] for other commit failures.
    pub fn delete(&self, key: &T::PrimaryKey) -> Result<()> {
        let transaction = self.core.__transaction();
        WriteTable::<T>::__new(transaction.clone(), self.table_id).delete(key)?;
        self.core.__commit(transaction)?;
        Ok(())
    }

    /// Deletes all listed primary keys atomically.
    ///
    /// Empty input makes no commit. Repeated keys are rejected as missing on the
    /// second occurrence, and no deletion is published on error.
    ///
    /// # Errors
    ///
    /// Returns [`crate::Error::NotFound`] if any key is absent or repeated, or
    /// [`crate::Error::WriteConflict`] for a concurrent write to any listed key.
    /// See [`Table`] for other commit failures.
    pub fn delete_many<I>(&self, keys: I) -> Result<()>
    where
        I: IntoIterator<Item = T::PrimaryKey>,
    {
        let transaction = self.core.__transaction();
        let table = WriteTable::<T>::__new(transaction.clone(), self.table_id);
        for key in keys {
            table.delete(&key)?;
        }
        self.core.__commit(transaction)?;
        Ok(())
    }

    /// Iterates all rows from one captured root.
    ///
    /// The root is captured now, rather than on the first call to `next`. Results
    /// use logical-row order as described by [`AllQuery`]. The iterator retains
    /// that root until dropped, including after it is exhausted.
    #[must_use]
    pub fn iter(&self) -> AllQuery<T> {
        AllQuery::new(self.core.lease_current(), self.table_id)
    }

    /// Current database-wide metrics.
    ///
    /// Includes every table sharing this database; see [`MetricsSnapshot`] for
    /// observational counter limitations.
    #[must_use]
    pub fn metrics(&self) -> MetricsSnapshot {
        self.core.metrics()
    }

    /// Looks up a generated unique index.
    #[doc(hidden)]
    #[must_use]
    pub fn __unique_index(&self, index_id: usize, key: &IndexKey) -> Option<RecordRef<T>> {
        self.core
            .with_current(|root| unique_from_root(root, self.table_id, index_id, key))
    }

    /// Creates a generated lazy index query.
    #[doc(hidden)]
    #[must_use]
    pub fn __index_query(
        &self,
        index_id: usize,
        lower: Bound<IndexKey>,
        upper: Bound<IndexKey>,
        prefix: Option<IndexKey>,
    ) -> IndexQuery<T> {
        IndexQuery::new(
            self.core.lease_current(),
            self.table_id,
            index_id,
            lower,
            upper,
            prefix,
        )
    }
}

impl<T: TableSchema> IntoIterator for &Table<T> {
    type Item = RecordRef<T>;
    type IntoIter = AllQuery<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

fn unique_from_root<T: TableSchema>(
    root: &crate::engine::DatabaseRoot,
    table_id: usize,
    index_id: usize,
    key: &IndexKey,
) -> Option<RecordRef<T>> {
    let table = root.table::<T>(table_id);
    let row_id = table
        .indexes
        .get(index_id)?
        .entries
        .get(key)?
        .iter()
        .next()?;
    table.rows.get(row_id).cloned().map(RecordRef::new)
}

/// A table fixed to one database-wide snapshot.
///
/// Generated database snapshots expose one of these handles per table. Clones
/// share the same root lease; later commits cannot change their visible rows.
/// Keeping this handle alive retains all tables reachable from the captured root.
#[derive(Clone, Debug)]
pub struct SnapshotTable<T: TableSchema> {
    lease: SnapshotLease,
    table_id: usize,
    marker: std::marker::PhantomData<fn() -> T>,
}

impl<T: TableSchema> SnapshotTable<T> {
    /// Creates a generated snapshot table facade.
    #[doc(hidden)]
    #[must_use]
    pub fn __new(lease: SnapshotLease, table_id: usize) -> Self {
        Self {
            lease,
            table_id,
            marker: std::marker::PhantomData,
        }
    }

    /// Snapshot commit sequence.
    ///
    /// Every table in the same generated snapshot reports the same sequence.
    #[must_use]
    pub fn sequence(&self) -> u64 {
        self.lease.sequence()
    }

    /// Primary-key lookup against this snapshot.
    ///
    /// Returns `None` for an absent key. The returned handle owns a reference to
    /// that row version and can outlive both the snapshot and the database.
    #[must_use]
    pub fn get(&self, key: &T::PrimaryKey) -> Option<RecordRef<T>> {
        self.lease
            .root()
            .table::<T>(self.table_id)
            .get(key)
            .map(RecordRef::new)
    }

    /// Explicitly clones a record visible to this snapshot.
    ///
    /// Returns `None` for an absent key; otherwise calls `T::clone`.
    #[must_use]
    pub fn get_cloned(&self, key: &T::PrimaryKey) -> Option<T> {
        self.get(key).map(|record| (*record).clone())
    }

    /// Iterates all rows visible to this snapshot.
    ///
    /// Uses [`AllQuery`]'s logical-row ordering and shares this snapshot's lease.
    #[must_use]
    pub fn iter(&self) -> AllQuery<T> {
        AllQuery::new(self.lease.clone(), self.table_id)
    }

    /// Looks up a generated unique index.
    #[doc(hidden)]
    #[must_use]
    pub fn __unique_index(&self, index_id: usize, key: &IndexKey) -> Option<RecordRef<T>> {
        unique_from_root(self.lease.root(), self.table_id, index_id, key)
    }

    /// Creates a generated lazy index query.
    #[doc(hidden)]
    #[must_use]
    pub fn __index_query(
        &self,
        index_id: usize,
        lower: Bound<IndexKey>,
        upper: Bound<IndexKey>,
        prefix: Option<IndexKey>,
    ) -> IndexQuery<T> {
        IndexQuery::new(
            self.lease.clone(),
            self.table_id,
            index_id,
            lower,
            upper,
            prefix,
        )
    }
}

impl<T: TableSchema> IntoIterator for &SnapshotTable<T> {
    type Item = RecordRef<T>;
    type IntoIter = AllQuery<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// A generated typed table inside an explicit write transaction.
///
/// Mutations are staged until the generated transaction closure returns `Ok` and
/// its commit succeeds. Reads performed internally by these methods observe prior
/// staged writes before the transaction's base snapshot. Unique indexes and write
/// conflicts are checked at commit rather than by each staging method.
///
/// An error from one staging operation does not automatically clear earlier
/// staged changes. Propagate errors with `?` from the transaction closure to abort
/// the complete transaction. If an error is caught and the closure returns `Ok`,
/// earlier successful staging operations can still commit. Clones share the same
/// staging buffer and cannot be sent or shared across threads.
#[derive(Clone)]
pub struct WriteTable<T: TableSchema> {
    transaction: TransactionContext,
    table_id: usize,
    marker: std::marker::PhantomData<fn() -> T>,
}

impl<T: TableSchema> std::fmt::Debug for WriteTable<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WriteTable")
            .field("name", &T::metadata().name)
            .finish_non_exhaustive()
    }
}

impl<T: TableSchema> WriteTable<T> {
    /// Creates a generated write-table facade.
    #[doc(hidden)]
    #[must_use]
    pub fn __new(transaction: TransactionContext, table_id: usize) -> Self {
        Self {
            transaction,
            table_id,
            marker: std::marker::PhantomData,
        }
    }

    /// Stages an insert.
    ///
    /// # Errors
    ///
    /// Returns [`crate::Error::DuplicatePrimaryKey`] if the key exists in a prior
    /// staged write or the base snapshot, unless a prior staged delete removed it.
    /// This operation does not yet validate unique secondary indexes.
    pub fn insert(&self, value: T) -> Result<()> {
        self.transaction
            .with_pending::<T, _>(self.table_id, |pending, base| pending.insert(base, value))
    }

    /// Stages multiple inserts.
    ///
    /// Stops at the first error. Earlier input rows remain staged; propagate the
    /// error out of the transaction closure to discard them.
    ///
    /// # Errors
    ///
    /// Returns [`crate::Error::DuplicatePrimaryKey`] under [`Self::insert`]'s rules,
    /// including repeated keys within this input.
    pub fn insert_many<I>(&self, values: I) -> Result<()>
    where
        I: IntoIterator<Item = T>,
    {
        for value in values {
            self.insert(value)?;
        }
        Ok(())
    }

    /// Stages an iterable using the same operation as [`Self::insert_many`].
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::insert_many`]. Earlier successful inserts
    /// remain staged if the caller catches the error.
    pub fn extend<I>(&self, values: I) -> Result<()>
    where
        I: IntoIterator<Item = T>,
    {
        self.insert_many(values)
    }

    /// Stages an insert or replacement.
    ///
    /// Replaces any prior staged value for this primary key. Repeated calls use
    /// the last value; unique-index conflicts can still reject the later commit.
    pub fn upsert(&self, value: T) {
        self.transaction
            .with_pending::<T, _>(self.table_id, |pending, base| pending.upsert(base, value));
    }

    /// Stages an update of a prior staged value or a row from the base snapshot.
    ///
    /// Calls the closure once on a cloned value, then replaces the staged value
    /// only if its primary key is unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`crate::Error::NotFound`] when the key is absent (including a prior
    /// staged delete), or [`crate::Error::PrimaryKeyChanged`] when the closure
    /// changes it. A failure leaves this key's previous staged value unchanged.
    ///
    /// # Panics
    ///
    /// Panics if the closure reenters a write method using any table in this same
    /// transaction: the staging buffer is already mutably borrowed.
    pub fn update<F>(&self, key: &T::PrimaryKey, update: F) -> Result<()>
    where
        F: FnOnce(&mut T),
    {
        self.transaction
            .with_pending::<T, _>(self.table_id, |pending, base| {
                pending.update(base, key, update)
            })
    }

    /// Stages a delete.
    ///
    /// Uses the prior staged value when present, otherwise the base snapshot, to
    /// decide whether the key exists.
    ///
    /// # Errors
    ///
    /// Returns [`crate::Error::NotFound`] if the key is absent, including when an
    /// earlier staged delete already removed it. No commit is attempted here.
    pub fn delete(&self, key: &T::PrimaryKey) -> Result<()> {
        self.transaction
            .with_pending::<T, _>(self.table_id, |pending, base| pending.delete(base, key))
    }
}
