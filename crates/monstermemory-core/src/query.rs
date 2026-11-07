//! Snapshot-retaining record handles and lazy index iterators.

use crate::{engine::SnapshotLease, key::IndexKey, schema::TableSchema, storage::RowId};
use std::{
    iter::FusedIterator,
    ops::{Bound, Deref},
    sync::Arc,
};

/// A lightweight owned handle to one immutable record version.
///
/// Dereferences to `T`. Cloning the handle increments an [`Arc`] reference count
/// without cloning `T`; the row remains accessible after an update, deletion, or
/// dropping its database. A handle retains only its row, not the database root.
/// The schema's immutability contract is described by [`TableSchema`].
#[derive(Debug)]
pub struct RecordRef<T> {
    record: Arc<T>,
}

impl<T> RecordRef<T> {
    pub(crate) fn new(record: Arc<T>) -> Self {
        Self { record }
    }

    /// Consumes this handle and returns its existing shared record pointer.
    ///
    /// The record is not cloned and other handles continue to reference the same
    /// version. This does not grant exclusive ownership of `T`.
    #[must_use]
    pub fn into_arc(self) -> Arc<T> {
        self.record
    }
}

impl<T> Clone for RecordRef<T> {
    fn clone(&self) -> Self {
        Self {
            record: Arc::clone(&self.record),
        }
    }
}

impl<T> Deref for RecordRef<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.record
    }
}

/// A lazy, stable-order iterator over a secondary or range index.
///
/// Generated index methods capture their database root when creating the query,
/// so subsequent commits do not alter its results. It yields [`RecordRef`] values
/// in ascending encoded-key order, then logical-row order for equal keys. Range
/// bounds supplied by the generated method retain their inclusive or exclusive
/// meaning. Composite prefixes match whole components rather than string prefixes.
///
/// A query retains the captured root until dropped, even after exhaustion. Use
/// ordinary [`Iterator`] adapters to limit or collect results. Collected record
/// handles can outlive the query without retaining its entire root.
pub struct IndexQuery<T: TableSchema> {
    lease: SnapshotLease,
    table_id: usize,
    index_id: usize,
    lower: Bound<IndexKey>,
    upper: Bound<IndexKey>,
    required_prefix: Option<IndexKey>,
    cursor_key: Option<IndexKey>,
    cursor_row: Option<RowId>,
    exhausted: bool,
    marker: std::marker::PhantomData<fn() -> T>,
}

impl<T: TableSchema> std::fmt::Debug for IndexQuery<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IndexQuery")
            .field("commit", &self.lease.sequence())
            .field("table_id", &self.table_id)
            .field("index_id", &self.index_id)
            .finish_non_exhaustive()
    }
}

impl<T: TableSchema> IndexQuery<T> {
    pub(crate) fn new(
        lease: SnapshotLease,
        table_id: usize,
        index_id: usize,
        lower: Bound<IndexKey>,
        upper: Bound<IndexKey>,
        required_prefix: Option<IndexKey>,
    ) -> Self {
        Self {
            lease,
            table_id,
            index_id,
            lower,
            upper,
            required_prefix,
            cursor_key: None,
            cursor_row: None,
            exhausted: false,
            marker: std::marker::PhantomData,
        }
    }
}

impl<T: TableSchema> Iterator for IndexQuery<T> {
    type Item = RecordRef<T>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.exhausted {
            return None;
        }

        let candidate = {
            let table = self.lease.root().table::<T>(self.table_id);
            let index = table.indexes.get(self.index_id)?;

            let mut found = None;
            if let (Some(key), Some(row_id)) = (&self.cursor_key, self.cursor_row)
                && let Some(rows) = index.entries.get(key)
                && let Some(next_row) = rows
                    .range((Bound::Excluded(row_id), Bound::Unbounded))
                    .next()
                && let Some(record) = table.rows.get(next_row)
            {
                found = Some((key.clone(), *next_row, Arc::clone(record)));
            }

            if found.is_none() {
                let lower = self
                    .cursor_key
                    .as_ref()
                    .map_or_else(|| self.lower.clone(), |key| Bound::Excluded(key.clone()));
                if let Some((key, rows)) = index.entries.range((lower, self.upper.clone())).next() {
                    if self
                        .required_prefix
                        .as_ref()
                        .is_some_and(|prefix| !key.starts_with(prefix))
                    {
                        None
                    } else {
                        rows.iter().next().and_then(|row_id| {
                            table
                                .rows
                                .get(row_id)
                                .map(|record| (key.clone(), *row_id, Arc::clone(record)))
                        })
                    }
                } else {
                    None
                }
            } else {
                found
            }
        };

        if let Some((key, row_id, record)) = candidate {
            self.cursor_key = Some(key);
            self.cursor_row = Some(row_id);
            Some(RecordRef::new(record))
        } else {
            self.exhausted = true;
            None
        }
    }
}

impl<T: TableSchema> FusedIterator for IndexQuery<T> {}

/// A lazy iterator over all live rows in stable logical-row order.
///
/// Created by [`crate::Table::iter`] or [`crate::SnapshotTable::iter`]. Iteration
/// observes one captured root and does not sort by primary key. New rows receive
/// monotonically increasing logical slots; replacing an existing row retains its
/// slot. Reinserting a row after a committed deletion gives it a new slot.
///
/// The captured root remains retained until this iterator is dropped, including
/// after exhaustion. Each yielded [`RecordRef`] independently retains its row.
pub struct AllQuery<T: TableSchema> {
    lease: SnapshotLease,
    table_id: usize,
    cursor: Option<RowId>,
    marker: std::marker::PhantomData<fn() -> T>,
}

impl<T: TableSchema> std::fmt::Debug for AllQuery<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AllQuery")
            .field("commit", &self.lease.sequence())
            .field("table_id", &self.table_id)
            .finish_non_exhaustive()
    }
}

impl<T: TableSchema> AllQuery<T> {
    pub(crate) fn new(lease: SnapshotLease, table_id: usize) -> Self {
        Self {
            lease,
            table_id,
            cursor: None,
            marker: std::marker::PhantomData,
        }
    }
}

impl<T: TableSchema> Iterator for AllQuery<T> {
    type Item = RecordRef<T>;

    fn next(&mut self) -> Option<Self::Item> {
        let table = self.lease.root().table::<T>(self.table_id);
        let next = match self.cursor {
            Some(row_id) => table
                .rows
                .range((Bound::Excluded(row_id), Bound::Unbounded))
                .next(),
            None => table.rows.iter().next(),
        }?;
        self.cursor = Some(*next.0);
        Some(RecordRef::new(Arc::clone(next.1)))
    }
}

impl<T: TableSchema> FusedIterator for AllQuery<T> {}
