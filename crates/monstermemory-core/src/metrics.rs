//! Low-overhead engine metrics.

use std::sync::atomic::{AtomicU64, Ordering};
use std::{
    collections::BTreeMap,
    sync::{Mutex, MutexGuard},
    time::{Duration, Instant},
};

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// An observational view of database activity and retained snapshot leases.
///
/// Counters and the commit sequence are read separately, so concurrent writes can
/// produce values from slightly different moments. Index-maintenance and tombstone
/// counters are updated while preparing candidate roots and can include candidates
/// later rejected by uniqueness, validation, or persistence failures. Do not use
/// these counters to enforce application invariants or reconstruct database state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MetricsSnapshot {
    /// Latest published commit sequence.
    pub commit_sequence: u64,
    /// Number of registered root-retaining leases, including snapshots and queries.
    ///
    /// Clones sharing a lease count once. Temporary validation or persistence
    /// snapshots also register leases; individual record handles do not.
    pub active_snapshots: u64,
    /// Time since the oldest active lease was registered, or `None` without leases.
    ///
    /// This measures lease retention rather than the age of its root's commit.
    pub oldest_snapshot_age: Option<Duration>,
    /// Successfully published non-empty transactions, including bulk loads.
    pub transaction_commits: u64,
    /// Transactions rejected by first-committer-wins validation.
    pub transaction_conflicts: u64,
    /// Unique-index constraint failures.
    pub unique_violations: u64,
    /// Secondary-index key additions/removals attempted during candidate creation.
    ///
    /// This counts maintenance operations, not individual persistent tree nodes.
    pub changed_index_paths: u64,
    /// Observational tombstone counter maintained during candidate creation.
    ///
    /// Deletion increments it and reinsertion decrements it; failed candidates can
    /// make it differ from the tombstones in the currently published root.
    pub tombstones: u64,
}

#[derive(Debug, Default)]
pub(crate) struct Metrics {
    commits: AtomicU64,
    conflicts: AtomicU64,
    unique_violations: AtomicU64,
    changed_index_paths: AtomicU64,
    tombstones: AtomicU64,
    next_snapshot: AtomicU64,
    snapshots: Mutex<BTreeMap<u64, Instant>>,
}

impl Metrics {
    pub(crate) fn register_snapshot(&self) -> u64 {
        let id = self.next_snapshot.fetch_add(1, Ordering::Relaxed) + 1;
        lock_unpoisoned(&self.snapshots).insert(id, Instant::now());
        id
    }

    pub(crate) fn unregister_snapshot(&self, id: u64) {
        lock_unpoisoned(&self.snapshots).remove(&id);
    }

    pub(crate) fn committed(&self) {
        self.commits.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn conflicted(&self) {
        self.conflicts.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn unique_violation(&self) {
        self.unique_violations.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn changed_index(&self) {
        self.changed_index_paths.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn tombstone_added(&self) {
        self.tombstones.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn tombstone_removed(&self) {
        let _ = self
            .tombstones
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_sub(1)
            });
    }

    pub(crate) fn snapshot(&self, commit_sequence: u64) -> MetricsSnapshot {
        let snapshots = lock_unpoisoned(&self.snapshots);
        let now = Instant::now();
        let oldest_snapshot_age = snapshots
            .values()
            .map(|start| now.duration_since(*start))
            .max();
        MetricsSnapshot {
            commit_sequence,
            active_snapshots: snapshots.len() as u64,
            oldest_snapshot_age,
            transaction_commits: self.commits.load(Ordering::Relaxed),
            transaction_conflicts: self.conflicts.load(Ordering::Relaxed),
            unique_violations: self.unique_violations.load(Ordering::Relaxed),
            changed_index_paths: self.changed_index_paths.load(Ordering::Relaxed),
            tombstones: self.tombstones.load(Ordering::Relaxed),
        }
    }
}
