# Dependency review

The versions below match `Cargo.lock` as reviewed on 2026-11-08. Ordinary derived-table use does not require importing the engine's collection dependencies. Custom `IndexValue`/`Lookup` implementations do expose `smallvec::SmallVec` in trait method signatures. Persistence exposes Serde bounds and `postcard::Error` through its public error enum.

This is an adoption inventory, not an independent security audit or a complete list of transitive licenses. License identifiers describe the upstream packages and do not replace their license/notice files. A binary redistribution must also account for the dependencies actually linked for its selected features and target. The workspace's declared MSRV is Rust 1.88; the resolved dependency graph must be checked with that toolchain rather than inferred from the direct-dependency table.

| Dependency | Reason | Public exposure / optional | MSRV impact | Unsafe / license | Alternatives considered |
|---|---|---|---|---|---|
| arc-swap 1.9.2 | Atomic `Arc<DatabaseRoot>` publication without reader OS lock | internal / required | verify resolved graph with workspace MSRV | upstream contains unsafe; MIT OR Apache-2.0 | `RwLock<Arc<_>>` violates primary-read design; custom atomic RC rejected |
| atomic-write-file 0.3.1 | Cross-platform atomic checkpoint replacement, including Windows | internal / persistence | verify resolved graph with workspace MSRV | platform implementation uses unsafe; BSD-3-Clause | direct filesystem replacement would need platform-specific handling |
| hashbrown 0.17.1 | SwissTable + raw-entry lookup lets chunk selection and lookup share one hash | internal / required | 1.85 | upstream unsafe; MIT OR Apache-2.0 | std HashMap double-hashed chunk lookup was slower |
| im 15.1.0 | Path-copy ordered maps/sets for row directory and range indexes | internal / required | 1.46 | upstream unsafe possible; MPL-2.0+ | full BTreeMap clone rejected; custom tree too risky for alpha |
| smallvec 1.15.2 | Inline index keys avoid heap allocation for common fields | public `IndexValue`/`Lookup` method signatures / required | verify resolved graph with workspace MSRV | upstream unsafe; MIT OR Apache-2.0 | Vec allocates; fixed maximum rejected |
| thiserror 2.0.20 | Concise source-preserving non-generic errors | derived impl only / required | 1.61+ | proc macro; MIT OR Apache-2.0 | handwritten Display/source |
| syn 2.0.119, quote 1.0.47, proc-macro2 1.0.107 | Span-preserving derive parser/emitter | compile-time only / required | syn 2 supports older than MSRV | proc macro ecosystem; MIT OR Apache-2.0 | manual TokenTree parser degrades diagnostics |
| serde 1.0.229 | User-controlled stable data model | bounds only / persistence | 1.56 | derive macro; MIT OR Apache-2.0 | bespoke codecs per record |
| postcard 1.1.3 | Portable payload encoding independent of Rust memory layout | `Error::Encode(postcard::Error)` in persistence API / persistence | verify resolved graph with workspace MSRV | dependency code is outside workspace unsafe prohibition; MIT OR Apache-2.0 | bincode/JSON; postcard is compact and format-version wrapped |
| crc32fast 1.5.0 | Accidental corruption/torn payload detection | internal / persistence | 1.63 | SIMD implementation may use unsafe; MIT OR Apache-2.0 | handwritten CRC; cryptographic MAC is separate security feature |

Dev-only: `proptest` differential testing, `trybuild` compile diagnostics, `loom` reduced concurrency model, `allocation-counter` warm-read allocation assertion. Fuzzing uses `libfuzzer-sys` only in the excluded `fuzz` package. No async runtime or global allocator is a production dependency.

The persistence container, rather than postcard itself, applies the encoded payload length limit before allocating its read buffer. Deserialization can allocate additional memory. Checkpoint replacement uses `atomic-write-file`'s [documented temporary-file commit](https://docs.rs/atomic-write-file/0.3.1/atomic_write_file/struct.AtomicWriteFile.html#method.commit), which synchronizes the temporary file before replacement for every `Durability` mode. WAL appends use a separate file path and explicitly call `sync_all` only for `Fsync`.
