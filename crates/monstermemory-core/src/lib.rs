//! # `MonsterMemory` core
//!
//! Safe MVCC storage and query types used by the `monstermemory` facade.
//!
//! An Attraqualia project, developed by halnos.
//!
//! The current implementation uses atomic `Arc` publication and persistent
//! collections. Published roots are immutable; readers never acquire the commit
//! mutex, and retained snapshots keep every reachable record and index node alive.
//!
//! Applications normally derive their schema through `monstermemory`; this crate
//! supplies the [`Table`], [`SnapshotTable`], and [`WriteTable`] handles returned by
//! those generated facades. [`RecordRef`] keeps an individual row version alive,
//! while [`AllQuery`] and [`IndexQuery`] retain an entire captured database root.
//! Writes use snapshot isolation with first-committer-wins validation for each
//! written primary key, rather than serializable read-set validation.
//!
//! # Index encoding
//!
//! [`IndexValue`] and [`Lookup`] define matching stored and borrowed key encodings.
//! Built-in integer encodings preserve numeric order, so composite keys can compare
//! each component without requiring the original records:
//!
//! ```
//! use monstermemory_core::{IndexKey, KeyPart, RetryPolicy};
//!
//! let country = KeyPart::from_lookup::<String, str>("JP");
//! let prefix = IndexKey::single(country.clone());
//! let younger = IndexKey::from_parts([country.clone(), KeyPart::from_value(&20_u16)]);
//! let older = IndexKey::from_parts([country, KeyPart::from_value(&30_u16)]);
//! assert!(younger < older);
//! assert!(older.starts_with(&prefix));
//! assert_eq!(RetryPolicy::limited(0).attempts(), 1);
//! ```
//!
//! This crate has no optional features. Persistence and schema derivation are
//! provided by the other `MonsterMemory` crates. The alpha API may change.

#![forbid(unsafe_code)]
#![deny(unsafe_op_in_unsafe_fn)]

mod engine;
mod error;
mod key;
mod metrics;
mod query;
mod schema;
mod storage;
mod table;

pub use engine::{
    __empty_table_root, DatabaseCore, DatabaseRoot, RetryPolicy, SnapshotLease, TransactionContext,
};
pub use error::{Error, Result};
pub use key::{IndexKey, IndexValue, KeyPart, Lookup};
pub use metrics::MetricsSnapshot;
pub use query::{AllQuery, IndexQuery, RecordRef};
pub use schema::{IndexDefinition, IndexKind, PrimaryKey, TableMetadata, TableSchema};
pub use table::{SnapshotTable, Table, WriteTable};
