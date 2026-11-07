//! Public, non-generic error model.

use std::io;

/// Result type used by `MonsterMemory` operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors returned by the storage engine.
///
/// The enum is non-exhaustive: callers should include a fallback match arm.
/// Only [`Self::WriteConflict`] is classified as retryable by [`Self::is_retryable`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// An insert used a primary key that is already visible.
    #[error("duplicate primary key in table `{table}`")]
    DuplicatePrimaryKey {
        /// Logical table name.
        table: &'static str,
    },
    /// A unique secondary index would contain more than one row.
    #[error("unique index `{index}` on table `{table}` would contain a duplicate")]
    UniqueIndexViolation {
        /// Logical table name.
        table: &'static str,
        /// Logical index name.
        index: &'static str,
    },
    /// The requested row is absent from the transaction's effective staged/base view.
    #[error("record not found in table `{table}`")]
    NotFound {
        /// Logical table name.
        table: &'static str,
    },
    /// Another transaction committed a write to the same primary key first.
    #[error(
        "write conflict in table `{table}` (transaction base {base_commit}, current {current_commit})"
    )]
    WriteConflict {
        /// Logical table name.
        table: &'static str,
        /// Commit observed when the transaction started.
        base_commit: u64,
        /// Commit current during validation.
        current_commit: u64,
    },
    /// An update closure changed the primary key.
    #[error("an update attempted to change the primary key in table `{table}`")]
    PrimaryKeyChanged {
        /// Logical table name.
        table: &'static str,
    },
    /// A checkpoint belongs to a different database schema.
    #[error("schema mismatch (expected {expected:#018x}, found {actual:#018x})")]
    SchemaMismatch {
        /// Fingerprint compiled into the application.
        expected: u64,
        /// Fingerprint stored in the checkpoint.
        actual: u64,
    },
    /// Persistence input failed structural or checksum validation.
    #[error("corrupt data in section `{section}` at byte {offset}: {message}")]
    CorruptData {
        /// Byte offset associated with the failure.
        offset: u64,
        /// Container section associated with the failure.
        section: &'static str,
        /// Concise, non-sensitive explanation.
        message: String,
    },
    /// An operating-system I/O operation failed.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    /// A builder option or runtime configuration is invalid.
    #[error("invalid configuration: {0}")]
    Configuration(String),
}

impl Error {
    /// Returns whether retrying a newly prepared transaction may succeed.
    ///
    /// Returns `true` only for [`Self::WriteConflict`]. The caller must rerun the
    /// operation against a fresh snapshot and ensure repeating its closure and
    /// external side effects is acceptable. This does not guarantee retry success.
    ///
    /// # Examples
    ///
    /// ```
    /// use monstermemory_core::Error;
    /// let error = Error::WriteConflict {
    ///     table: "users",
    ///     base_commit: 1,
    ///     current_commit: 2,
    /// };
    /// assert!(error.is_retryable());
    /// assert!(!Error::NotFound { table: "users" }.is_retryable());
    /// ```
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        matches!(self, Self::WriteConflict { .. })
    }
}
