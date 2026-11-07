# Changelog

## [Unreleased]

### Fixed

- Explicitly select Rust 1.88.0 for the existing MSRV job and nightly for Miri/fuzz, so the repository's pinned development toolchain does not override them.
- Verify the compiler-generated trait-bound diagnostic against a full Rust 1.88 snapshot on the MSRV, while keeping the development-compiler snapshot and all compile-fail cases.

### Documentation

- Added runnable API examples and documented generated table and database APIs.
- Documented snapshot retention, transaction retries, metrics, schema fingerprints, memory settings, and persistence recovery limitations.
- Added contributor guidance and included both license texts in distributable crates.

### Added

- Initial alpha typed MVCC engine, derive macros, indexes, transactions, snapshots, persistence container, tests, benchmark, and Japanese manual.

