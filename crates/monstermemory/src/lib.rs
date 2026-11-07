//! # `MonsterMemory`
//!
//! An Attraqualia project, developed by halnos.
//!
//! An embedded typed MVCC in-memory database. Derive [`Table`](macro@Table) for
//! records and [`Database`] for a struct of table handles to generate index queries,
//! database-wide snapshots, and atomic write transactions. The types and traits
//! from `monstermemory-core` are re-exported here; [`prelude`] contains common imports.
//!
//! # Records, indexes, and transactions
//!
//! ```
//! use monstermemory::prelude::*;
//!
//! #[derive(Clone, Table)]
//! #[composite_index(name = "country_age", fields(country, age), kind = "ordered")]
//! struct User {
//!     #[primary_key]
//!     id: u64,
//!     #[index(unique)]
//!     email: String,
//!     #[index]
//!     country: String,
//!     #[index(range)]
//!     age: u16,
//! }
//!
//! #[derive(Database)]
//! struct AppDb {
//!     users: Table<User>,
//! }
//!
//! let db = AppDb::bulk_load(|loader| {
//!     loader.users.insert(User {
//!         id: 1,
//!         email: "alice@example.com".into(),
//!         country: "JP".into(),
//!         age: 28,
//!     })?;
//!     Ok(())
//! })?;
//! assert_eq!(db.users.by_email("alice@example.com").map(|row| row.id), Some(1));
//! assert_eq!(db.users.by_country("JP").count(), 1);
//! assert_eq!(db.users.age_range(20..30).count(), 1);
//! assert_eq!(db.users.by_country_age(("JP", 25..=30)).count(), 1);
//!
//! let before = db.snapshot();
//! db.write(|tx| {
//!     tx.users.update(&1, |user| user.age += 1)?;
//!     Ok(())
//! })?;
//! assert_eq!(before.users.get(&1).map(|user| user.age), Some(28));
//! assert_eq!(db.users.get(&1).map(|user| user.age), Some(29));
//!
//! // Retry only operations that are safe to execute more than once.
//! db.retrying_write(RetryPolicy::limited(3), |tx| {
//!     tx.users.update(&1, |user| user.age += 1)?;
//!     Ok(())
//! })?;
//! assert_eq!(db.users.get(&1).map(|user| user.age), Some(30));
//! # Ok::<(), monstermemory::Error>(())
//! ```
//!
//! A live [`Table::get`] observes the root current for that call; separate calls
//! can observe different commits. Use a generated `snapshot()` for a consistent
//! view across tables. [`RecordRef`] owns one immutable record version, while
//! [`AllQuery`] and [`IndexQuery`] retain a complete snapshot root until dropped.
//! Record types must not mutate shared interior state if that state is intended
//! to remain fixed in a snapshot.
//!
//! Writes use first-committer-wins validation for written primary keys and check
//! unique indexes before publication. Isolation is Snapshot Isolation; predicate
//! read sets are not tracked. The commit gate serializes writers. Ordinary writes
//! never retry a caller's closure; generated `retrying_write` retries commit-time
//! [`Error::WriteConflict`] only, within the supplied [`RetryPolicy`]. A closure's
//! own error aborts the operation. See [`Error`] and [`Error::is_retryable`].
//!
//! # Persistence and features
//!
//! Default features are empty and require no configuration or runtime service.
//! Enable `persistence` and add `#[database(persistence)]` to generate checkpoint
//! and WAL APIs. Records must implement Serde serialization and deserialization.
//! The `persistence` module documents the container, limits, and durability modes when the
//! feature is enabled. The repository's `persistence` example demonstrates these
//! generated methods; run it with `cargo run -p monstermemory --example persistence
//! --features persistence` from the checkout.
//!
//! # Alpha limitations
//!
//! The API and persistence format may change. The builder's `memory_limit_bytes`
//! setting currently rejects zero but does not enforce a memory limit;
//! `snapshot_warning_after` does not install monitoring. Snapshot/query creation
//! registers retention metrics under a mutex; the engine's primary lookup path
//! does not acquire the writer commit mutex. No serializable isolation, automatic
//! schema migration, cross-process writer coordination, or network server is provided.
//! The generated database fingerprint does not include every record field. After
//! recovering from an unclean shutdown, successfully checkpoint to the same path
//! before further writes: incomplete WAL tails are ignored but not repaired.

#![forbid(unsafe_code)]

pub use monstermemory_core::*;
pub use monstermemory_macros::{Database, Table};

/// Checkpoint and WAL support enabled by the `persistence` feature (alpha format).
#[cfg(feature = "persistence")]
pub use monstermemory_persistence as persistence;

/// Implementation details referenced by generated code. Not a stable API.
#[doc(hidden)]
pub mod __private {
    pub use monstermemory_core::*;
}

/// Common imports for `MonsterMemory` applications.
pub mod prelude {
    pub use crate::{Database, Error, Result, RetryPolicy, Table};
}
