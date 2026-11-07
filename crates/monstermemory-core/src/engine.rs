//! Database-wide atomic publication and transaction coordination.

use crate::{
    error::{Error, Result},
    metrics::{Metrics, MetricsSnapshot},
    schema::TableSchema,
    storage::{PendingTable, TableRoot},
};
use arc_swap::ArcSwap;
use std::{
    any::Any,
    cell::RefCell,
    collections::BTreeMap,
    rc::Rc,
    sync::{Arc, Mutex, MutexGuard, OnceLock, Weak},
};

pub(crate) type AnyTableRoot = Arc<dyn Any + Send + Sync>;
type RootHook = Box<dyn Fn(Arc<DatabaseRoot>) -> Result<()> + Send + Sync>;

/// One immutable, database-wide MVCC root.
///
/// Every table version in the root belongs to the same atomic publication. Normal
/// application access uses generated snapshot facades instead of this engine type.
#[derive(Debug)]
pub struct DatabaseRoot {
    pub(crate) sequence: u64,
    pub(crate) schema_fingerprint: u64,
    pub(crate) tables: Vec<AnyTableRoot>,
}

impl DatabaseRoot {
    pub(crate) fn table<T: TableSchema>(&self, table_id: usize) -> &TableRoot<T> {
        self.tables[table_id]
            .downcast_ref::<TableRoot<T>>()
            .expect("generated table id and root type must agree")
    }

    /// Published commit sequence observed by this root.
    ///
    /// An empty in-memory database starts at zero. Non-empty successful commits
    /// advance the sequence once; this is not a timestamp or durable record ID.
    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Fingerprint of the generated database schema.
    #[must_use]
    pub const fn schema_fingerprint(&self) -> u64 {
        self.schema_fingerprint
    }
}

trait PendingCommit {
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn table_id(&self) -> usize;
    fn is_empty(&self) -> bool;
    fn validate(&self, current: &DatabaseRoot) -> Result<()>;
    fn apply(&self, current: &DatabaseRoot, commit: u64, metrics: &Metrics)
    -> Result<AnyTableRoot>;
}

impl<T: TableSchema> PendingCommit for PendingTable<T> {
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn table_id(&self) -> usize {
        self.table_id()
    }

    fn is_empty(&self) -> bool {
        self.is_empty()
    }

    fn validate(&self, current: &DatabaseRoot) -> Result<()> {
        self.validate(current.table::<T>(self.table_id()), current.sequence)
    }

    fn apply(
        &self,
        current: &DatabaseRoot,
        commit: u64,
        metrics: &Metrics,
    ) -> Result<AnyTableRoot> {
        Ok(Arc::new(self.apply(
            current.table::<T>(self.table_id()),
            commit,
            metrics,
        )?))
    }
}

#[derive(Default)]
struct TransactionBuffer {
    tables: BTreeMap<usize, Box<dyn PendingCommit>>,
}

struct TransactionInner {
    base: Arc<DatabaseRoot>,
    buffer: RefCell<TransactionBuffer>,
}

/// Shared transaction-local state used by generated typed write facades.
///
/// Clones share one staging buffer and the immutable base root. The context uses
/// thread-local reference counting and cannot be sent or shared between threads.
#[derive(Clone)]
pub struct TransactionContext {
    inner: Rc<TransactionInner>,
}

impl TransactionContext {
    pub(crate) fn new(base: Arc<DatabaseRoot>) -> Self {
        Self {
            inner: Rc::new(TransactionInner {
                base,
                buffer: RefCell::new(TransactionBuffer::default()),
            }),
        }
    }

    pub(crate) fn with_pending<T, R>(
        &self,
        table_id: usize,
        operation: impl FnOnce(&mut PendingTable<T>, &TableRoot<T>) -> R,
    ) -> R
    where
        T: TableSchema,
    {
        let base = self.inner.base.table::<T>(table_id);
        // 更新クロージャの実行中も借用を保持するため、同じトランザクションへの再入は禁止する。
        let mut buffer = self.inner.buffer.borrow_mut();
        let pending = buffer.tables.entry(table_id).or_insert_with(|| {
            Box::new(PendingTable::<T>::new(table_id, self.inner.base.sequence))
        });
        let typed = pending
            .as_any_mut()
            .downcast_mut::<PendingTable<T>>()
            .expect("generated transaction table id and type must agree");
        operation(typed, base)
    }

    fn take_pending(self) -> Vec<Box<dyn PendingCommit>> {
        std::mem::take(&mut self.inner.buffer.borrow_mut().tables)
            .into_values()
            .filter(|pending| !pending.is_empty())
            .collect()
    }
}

/// Thread-safe MVCC engine shared by all generated table handles.
///
/// Readers load a published immutable root; commits serialize validation and
/// publication through one mutex. Constructors and commit helpers are generated
/// code support and are not a stable application interface.
pub struct DatabaseCore {
    current: ArcSwap<DatabaseRoot>,
    commit_gate: Mutex<()>,
    metrics: Metrics,
    validator: OnceLock<RootHook>,
    commit_hook: OnceLock<RootHook>,
}

impl std::fmt::Debug for DatabaseCore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DatabaseCore")
            .field("sequence", &self.current.load().sequence)
            .finish_non_exhaustive()
    }
}

impl DatabaseCore {
    /// Creates an empty database from generated table roots.
    #[doc(hidden)]
    #[must_use]
    pub fn __memory(schema_fingerprint: u64, tables: Vec<AnyTableRoot>) -> Arc<Self> {
        Arc::new(Self {
            current: ArcSwap::from_pointee(DatabaseRoot {
                sequence: 0,
                schema_fingerprint,
                tables,
            }),
            commit_gate: Mutex::new(()),
            metrics: Metrics::default(),
            validator: OnceLock::new(),
            commit_hook: OnceLock::new(),
        })
    }

    fn lock_commit(&self) -> MutexGuard<'_, ()> {
        self.commit_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Captures the current immutable root for transaction preparation.
    #[doc(hidden)]
    #[must_use]
    pub fn __transaction(&self) -> TransactionContext {
        TransactionContext::new(self.current.load_full())
    }

    /// Validates and atomically publishes a prepared transaction.
    #[doc(hidden)]
    pub fn __commit(&self, transaction: TransactionContext) -> Result<u64> {
        self.__commit_validated(transaction, |_| Ok(()))
    }

    /// Publishes a transaction only if a generated cross-table validator accepts
    /// the candidate immutable root.
    #[doc(hidden)]
    pub fn __commit_validated<F>(&self, transaction: TransactionContext, validate: F) -> Result<u64>
    where
        F: FnOnce(Arc<DatabaseRoot>) -> Result<()>,
    {
        let pending = transaction.take_pending();
        if pending.is_empty() {
            return Ok(self.current.load().sequence);
        }

        let _gate = self.lock_commit();
        let current = self.current.load_full();
        for table in &pending {
            if let Err(error) = table.validate(&current) {
                if matches!(error, Error::WriteConflict { .. }) {
                    self.metrics.conflicted();
                }
                return Err(error);
            }
        }

        let next_sequence = current
            .sequence
            .checked_add(1)
            .ok_or_else(|| Error::Configuration("commit sequence exhausted".to_owned()))?;
        let mut tables = current.tables.clone();
        for table in &pending {
            tables[table.table_id()] = table.apply(&current, next_sequence, &self.metrics)?;
        }

        let next = Arc::new(DatabaseRoot {
            sequence: next_sequence,
            schema_fingerprint: current.schema_fingerprint,
            tables,
        });
        // 検証と永続化が完了する前に公開すると、失敗した書き込みがreaderから見えてしまう。
        if let Some(validator) = self.validator.get() {
            validator(Arc::clone(&next))?;
        }
        validate(Arc::clone(&next))?;
        if let Some(commit_hook) = self.commit_hook.get() {
            commit_hook(Arc::clone(&next))?;
        }
        self.current.store(next);
        self.metrics.committed();
        Ok(next_sequence)
    }

    pub(crate) fn current(&self) -> Arc<DatabaseRoot> {
        self.current.load_full()
    }

    pub(crate) fn with_current<R>(&self, operation: impl FnOnce(&DatabaseRoot) -> R) -> R {
        let root = self.current.load();
        operation(&root)
    }

    /// Creates an explicit root-retaining snapshot lease.
    #[doc(hidden)]
    #[must_use]
    pub fn __snapshot(self: &Arc<Self>) -> SnapshotLease {
        SnapshotLease::new(self, self.current())
    }

    /// Creates a validation-only lease for an unpublished candidate root.
    #[doc(hidden)]
    #[must_use]
    pub fn __lease_root(self: &Arc<Self>, root: Arc<DatabaseRoot>) -> SnapshotLease {
        SnapshotLease::new(self, root)
    }

    /// Installs the generated database validator before the core is published.
    #[doc(hidden)]
    pub fn __set_validator<F>(&self, validator: F) -> Result<()>
    where
        F: Fn(Arc<DatabaseRoot>) -> Result<()> + Send + Sync + 'static,
    {
        self.validator
            .set(Box::new(validator))
            .map_err(|_| Error::Configuration("database validator is already installed".to_owned()))
    }

    /// Installs an optional persistence hook called after validation and before publication.
    #[doc(hidden)]
    pub fn __set_commit_hook<F>(&self, commit_hook: F) -> Result<()>
    where
        F: Fn(Arc<DatabaseRoot>) -> Result<()> + Send + Sync + 'static,
    {
        self.commit_hook.set(Box::new(commit_hook)).map_err(|_| {
            Error::Configuration("database commit hook is already installed".to_owned())
        })
    }

    /// Runs checkpoint work against one root while excluding commits.
    #[doc(hidden)]
    pub fn __exclusive_current<R>(
        &self,
        operation: impl FnOnce(Arc<DatabaseRoot>) -> Result<R>,
    ) -> Result<R> {
        let _gate = self.lock_commit();
        operation(self.current.load_full())
    }

    pub(crate) fn lease_current(self: &Arc<Self>) -> SnapshotLease {
        self.__snapshot()
    }

    /// Returns current counters without instrumenting the read hot path.
    ///
    /// Values are observational and are not one atomic transactional snapshot; see
    /// [`MetricsSnapshot`] for counter limitations.
    #[must_use]
    pub fn metrics(&self) -> MetricsSnapshot {
        let sequence = self.current.load().sequence;
        self.metrics.snapshot(sequence)
    }
}

struct SnapshotInner {
    root: Arc<DatabaseRoot>,
    core: Weak<DatabaseCore>,
    metric_id: u64,
}

impl Drop for SnapshotInner {
    fn drop(&mut self) {
        if let Some(core) = self.core.upgrade() {
            core.metrics.unregister_snapshot(self.metric_id);
        }
    }
}

/// A cloneable lease retaining one immutable database root.
///
/// The root remains alive until the last clone of this lease is dropped, even if
/// the database itself is dropped. Cloning shares a metrics registration rather
/// than registering a separate snapshot. Generated facades create these leases.
#[derive(Clone)]
pub struct SnapshotLease {
    inner: Arc<SnapshotInner>,
}

impl std::fmt::Debug for SnapshotLease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SnapshotLease")
            .field("sequence", &self.inner.root.sequence)
            .finish_non_exhaustive()
    }
}

impl SnapshotLease {
    fn new(core: &Arc<DatabaseCore>, root: Arc<DatabaseRoot>) -> Self {
        let metric_id = core.metrics.register_snapshot();
        Self {
            inner: Arc::new(SnapshotInner {
                root,
                core: Arc::downgrade(core),
                metric_id,
            }),
        }
    }

    pub(crate) fn root(&self) -> &Arc<DatabaseRoot> {
        &self.inner.root
    }

    /// Commit sequence consistently observed by this lease.
    #[must_use]
    pub fn sequence(&self) -> u64 {
        self.inner.root.sequence
    }
}

/// Creates a type-erased empty table root for generated database constructors.
#[doc(hidden)]
#[must_use]
pub fn __empty_table_root<T: TableSchema>() -> AnyTableRoot {
    Arc::new(TableRoot::<T>::empty())
}

/// Explicit retry limit for closures that may safely be executed more than once.
///
/// Used by the generated database retry method. Only write conflicts are retried;
/// callers must make closure side effects safe to repeat. This policy specifies
/// an attempt count and does not provide a delay or exponential backoff.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetryPolicy {
    attempts: u32,
}

impl RetryPolicy {
    /// Creates a policy allowing at most `attempts` executions, including the first.
    ///
    /// Zero is normalized to one, so the initial operation is always attempted.
    ///
    /// # Examples
    ///
    /// ```
    /// use monstermemory_core::RetryPolicy;
    /// assert_eq!(RetryPolicy::limited(3).attempts(), 3);
    /// assert_eq!(RetryPolicy::limited(0).attempts(), 1);
    /// ```
    #[must_use]
    pub const fn limited(attempts: u32) -> Self {
        Self {
            attempts: if attempts == 0 { 1 } else { attempts },
        }
    }

    /// Maximum closure executions.
    #[must_use]
    pub const fn attempts(self) -> u32 {
        self.attempts
    }
}
