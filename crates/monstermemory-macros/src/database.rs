use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use std::collections::HashMap;
use syn::{
    Data, DeriveInput, Error, Fields, GenericArgument, Ident, Path, PathArguments, Type,
    spanned::Spanned as _,
};

struct TableField {
    ident: Ident,
    record: Type,
}

#[derive(Default)]
struct DatabaseOptions {
    validator: Option<Path>,
    persistence: bool,
}

#[allow(
    clippy::too_many_lines,
    reason = "generated facade is kept in one quote tree for reviewability"
)]
pub(crate) fn expand(input: TokenStream) -> syn::Result<TokenStream> {
    let input: DeriveInput = syn::parse2(input)?;
    if !input.generics.params.is_empty() {
        return Err(Error::new_spanned(
            &input.generics,
            "MonsterMemory databases cannot be generic or contain lifetimes",
        ));
    }
    let Data::Struct(data) = &input.data else {
        return Err(Error::new_spanned(
            &input.ident,
            "Database can only be derived for a struct",
        ));
    };
    let Fields::Named(fields) = &data.fields else {
        return Err(Error::new_spanned(
            &data.fields,
            "Database requires named Table<T> fields",
        ));
    };
    if fields.named.is_empty() {
        return Err(Error::new_spanned(
            &input.ident,
            "Database requires at least one Table<T> field",
        ));
    }

    let mut table_fields = Vec::new();
    let mut records = HashMap::<String, proc_macro2::Span>::new();
    for field in &fields.named {
        let ident = field.ident.clone().expect("named field");
        let record = table_record_type(&field.ty)?;
        let record_text = quote!(#record).to_string();
        if let Some(previous) = records.insert(record_text.clone(), field.ty.span()) {
            let mut error = Error::new_spanned(
                &field.ty,
                format!("the same table record type `{record_text}` is registered twice"),
            );
            error.combine(Error::new(previous, "the first registration is here"));
            return Err(error);
        }
        table_fields.push(TableField { ident, record });
    }
    let options = parse_database_attributes(&input.attrs)?;

    let ident = &input.ident;
    let visibility = &input.vis;
    let snapshot_ident = format_ident!("{}Snapshot", ident);
    let transaction_ident = format_ident!("{}WriteTransaction", ident);
    let bulk_ident = format_ident!("{}BulkLoader", ident);
    let builder_ident = format_ident!("{}Builder", ident);
    let first_field = &table_fields[0].ident;
    let fingerprint_material = format!(
        "{}:{}",
        ident,
        table_fields
            .iter()
            .map(|field| {
                let name = &field.ident;
                let record = &field.record;
                format!("{}={}", name, quote!(#record))
            })
            .collect::<Vec<_>>()
            .join(";")
    );
    let fingerprint = fnv1a(fingerprint_material.as_bytes());

    let field_names: Vec<_> = table_fields.iter().map(|field| &field.ident).collect();
    let record_types: Vec<_> = table_fields.iter().map(|field| &field.record).collect();
    // Constructors, snapshots, and persistence images all use declaration order as table IDs.
    // Reordering only one of these lists would connect a facade to the wrong erased root.
    let table_ids: Vec<_> = (0..table_fields.len()).map(syn::Index::from).collect();

    let commit = quote!(core.__commit(context));

    let commit_retry = commit.clone();
    let validator_install = options.validator.as_ref().map(|path| {
        quote! {
            // The core owns this callback; a weak capture avoids retaining itself forever.
            let validator_core = ::std::sync::Arc::downgrade(&core);
            core.__set_validator(move |root| {
                let validator_core = validator_core.upgrade().ok_or_else(|| {
                    ::monstermemory::Error::Configuration(
                        "database core was dropped during validation".to_owned(),
                    )
                })?;
                let lease = validator_core.__lease_root(root);
                let candidate = #snapshot_ident::__new(lease);
                #path(&candidate)
            }).expect("generated validator is installed exactly once");
        }
    });
    let persistence_methods = if options.persistence {
        let image_tables = field_names
            .iter()
            .zip(&record_types)
            .map(|(field, record)| {
                quote! {
                    ::monstermemory::persistence::encode_table(
                        &snapshot.#field.iter()
                            .map(|row| (*row).clone())
                            .collect::<::std::vec::Vec<#record>>()
                    )?
                }
            });
        let restored_tables = field_names
            .iter()
            .zip(record_types.iter().zip(&table_ids))
            .map(|(field, (record, table_id))| {
                quote! {
                    loader.#field.insert_many(
                        ::monstermemory::persistence::decode_table::<::std::vec::Vec<#record>>(
                            &image.tables[#table_id]
                        )?
                    )?;
                }
            });
        let table_count = table_fields.len();
        quote! {
            fn __encode_image(
                snapshot: &#snapshot_ident,
            ) -> ::monstermemory::Result<::monstermemory::persistence::DatabaseImage> {
                Ok(::monstermemory::persistence::DatabaseImage {
                    tables: vec![#(#image_tables),*],
                })
            }

            fn __install_wal(&self, checkpoint_path: ::std::path::PathBuf) -> ::monstermemory::Result<()> {
                let core = self.#first_field.__core();
                let persistence_core = ::std::sync::Arc::downgrade(&core);
                let wal_path = ::monstermemory::persistence::wal_path(&checkpoint_path);
                core.__set_commit_hook(move |root| {
                    let persistence_core = persistence_core.upgrade().ok_or_else(|| {
                        ::monstermemory::Error::Configuration(
                            "database core was dropped during WAL append".to_owned(),
                        )
                    })?;
                    let snapshot = #snapshot_ident::__new(persistence_core.__lease_root(root));
                    let image = Self::__encode_image(&snapshot)?;
                    ::monstermemory::persistence::append_wal(
                        &wal_path,
                        #fingerprint,
                        &image,
                        ::monstermemory::persistence::Durability::Fsync,
                    )?;
                    Ok(())
                })
            }

            /// Restores a checkpoint and its sidecar WAL, then enables synchronous WAL logging.
            ///
            /// The checkpoint is validated first if it exists. The latest complete WAL image
            /// takes precedence over a valid checkpoint; an absent checkpoint and absent or
            /// header-only WAL produce an empty database. Incomplete WAL tails are ignored, but
            /// structurally invalid headers and complete corrupt frames are rejected.
            /// Recovered rows pass the normal bulk-load uniqueness and database validation.
            /// Recovery does not truncate incomplete tails. After an unclean shutdown,
            /// successfully checkpoint to this same path before allowing any further writes;
            /// appending behind an unrepaired tail can make later commits unrecoverable.
            ///
            /// Each later nonempty commit writes a full database image to `<path>.wal` using
            /// `Durability::Fsync` before publishing its root. The caller must ensure exclusive
            /// ownership of this checkpoint/WAL pair; file locking across instances is absent.
            ///
            /// # Errors
            ///
            /// Returns schema, corruption, size-limit, serialization, or I/O errors from
            /// recovery, and constraint or validator errors while loading recovered rows.
            /// A corrupt checkpoint is rejected even if a valid WAL exists beside it.
            /// The generated fingerprint does not detect every record-layout change; see
            /// [`Self::schema_fingerprint`] before reusing data after a schema edit.
            pub fn open(path: impl ::std::convert::AsRef<::std::path::Path>) -> ::monstermemory::Result<Self> {
                let path = path.as_ref();
                let checkpoint = match ::monstermemory::persistence::read_checkpoint(path, #fingerprint) {
                        Ok(image) => Some(image),
                        Err(::monstermemory::persistence::Error::Io(error))
                            if error.kind() == ::std::io::ErrorKind::NotFound => None,
                        Err(error) => return Err(error.into()),
                };
                let wal_path = ::monstermemory::persistence::wal_path(path);
                let recovered: ::core::option::Option<::monstermemory::persistence::DatabaseImage> =
                    ::monstermemory::persistence::read_latest_wal(
                    &wal_path,
                    #fingerprint,
                )?;
                let database = if let Some(image) = recovered.or(checkpoint) {
                    if image.tables.len() != #table_count {
                        return Err(::monstermemory::Error::CorruptData {
                            offset: 0,
                            section: "table directory",
                            message: "checkpoint table count does not match the compiled schema".to_owned(),
                        });
                    }
                    Self::bulk_load(|loader| {
                        #(#restored_tables)*
                        Ok(())
                    })?
                } else {
                    Self::memory()
                };
                database.__install_wal(path.to_path_buf())?;
                Ok(database)
            }

            /// Writes a checksummed checkpoint with an atomic temporary-file handoff.
            ///
            /// Uses `Durability::Fsync` and excludes commits while capturing and writing the
            /// current image. The sidecar WAL for this path is removed after replacement.
            /// This does not enable WAL logging on a database created with `memory()`.
            ///
            /// # Errors
            ///
            /// Returns payload encoding, file replacement, synchronization, or WAL-removal
            /// errors. If WAL removal fails, the checkpoint may already have been replaced.
            pub fn checkpoint(&self, path: impl ::std::convert::AsRef<::std::path::Path>) -> ::monstermemory::Result<()> {
                self.checkpoint_with_durability(
                    path,
                    ::monstermemory::persistence::Durability::Fsync,
                )
            }

            /// Writes a checkpoint using an explicit flush guarantee.
            ///
            /// Commits are excluded until the current image is written and this path's WAL is
            /// cleared. Passing another path does not redirect an existing instance's WAL hook.
            /// See `monstermemory::persistence::Durability` for the persistence layer's actual
            /// guarantees; checkpoint replacement may synchronize even for weaker policies.
            ///
            /// # Errors
            ///
            /// Returns payload encoding, checkpoint I/O/replacement, synchronization, or
            /// WAL-removal errors. A failure after replacement can leave a new checkpoint
            /// together with the old WAL. In-memory contents are not changed by this operation.
            pub fn checkpoint_with_durability(
                &self,
                path: impl ::std::convert::AsRef<::std::path::Path>,
                durability: ::monstermemory::persistence::Durability,
            ) -> ::monstermemory::Result<()> {
                let path = path.as_ref().to_path_buf();
                let core = self.#first_field.__core();
                core.__exclusive_current(|root| {
                    let snapshot = #snapshot_ident::__new(core.__lease_root(root));
                    let image = Self::__encode_image(&snapshot)?;
                    ::monstermemory::persistence::write_checkpoint(
                        &path,
                        #fingerprint,
                        &image,
                        durability,
                    )?;
                    ::monstermemory::persistence::clear_wal(
                        ::monstermemory::persistence::wal_path(&path),
                    )?;
                    Ok(())
                })
            }
        }
    } else {
        TokenStream::new()
    };
    let expanded = quote! {
        #[doc = concat!("A database-wide immutable snapshot of [`", stringify!(#ident), "`].")]
        ///
        /// Every table observes the same commit sequence. Keeping this value or an iterator
        /// created from it alive retains the reachable historical records and index nodes.
        #visibility struct #snapshot_ident {
            #(
                #[doc = concat!("Snapshot view of the `", stringify!(#field_names), "` table.")]
                pub #field_names: ::monstermemory::__private::SnapshotTable<#record_types>,
            )*
        }

        impl #snapshot_ident {
            fn __new(lease: ::monstermemory::__private::SnapshotLease) -> Self {
                Self {
                    #(#field_names: ::monstermemory::__private::SnapshotTable::__new(
                        lease.clone(), #table_ids,
                    ),)*
                }
            }

            /// Returns the database-wide commit sequence observed by every table.
            #[must_use]
            pub fn sequence(&self) -> u64 {
                self.#first_field.sequence()
            }
        }

        #[doc = concat!("A typed write transaction for [`", stringify!(#ident), "`].")]
        ///
        /// Table operations stage changes against one immutable base root. Publication is
        /// performed by the database's `write` or `retrying_write` method after the closure
        /// succeeds; dropping this value alone does not commit anything.
        #visibility struct #transaction_ident {
            #(
                #[doc = concat!("Transactional writer for the `", stringify!(#field_names), "` table.")]
                pub #field_names: ::monstermemory::__private::WriteTable<#record_types>,
            )*
        }

        #[doc = concat!("An atomic initial-data loader for [`", stringify!(#ident), "`].")]
        ///
        /// All tables share one pending commit in a newly created memory database. The
        /// database's `bulk_load` method publishes it only after the loader closure succeeds.
        #visibility struct #bulk_ident {
            #(
                #[doc = concat!("Bulk writer for the `", stringify!(#field_names), "` table.")]
                pub #field_names: ::monstermemory::__private::WriteTable<#record_types>,
            )*
        }

        #[doc = concat!("Advanced opt-in configuration for [`", stringify!(#ident), "`].")]
        ///
        /// The current alpha validates a zero memory budget but does not enforce positive
        /// budgets or emit snapshot-age warnings. No configuration is needed for `memory()`.
        #visibility struct #builder_ident {
            memory_limit_bytes: ::core::option::Option<usize>,
            snapshot_warning_after: ::std::time::Duration,
        }

        impl #builder_ident {
            /// Records a requested memory budget in bytes.
            ///
            /// `build` rejects zero. Positive values currently do not limit allocations or
            /// evict records; the application must enforce its own memory policy.
            #[must_use]
            pub const fn memory_limit_bytes(mut self, bytes: usize) -> Self {
                self.memory_limit_bytes = Some(bytes);
                self
            }

            /// Records a requested warning age for retained snapshots.
            ///
            /// The current implementation accepts this setting without emitting warnings.
            /// Use database metrics to implement application monitoring.
            #[must_use]
            pub const fn snapshot_warning_after(mut self, duration: ::std::time::Duration) -> Self {
                self.snapshot_warning_after = duration;
                self
            }

            /// Builds a memory-only database.
            ///
            /// # Errors
            ///
            /// Returns `monstermemory::Error::Configuration` if `memory_limit_bytes(0)` was
            /// supplied. Other stored settings currently do not change engine behavior.
            pub fn build(self) -> ::monstermemory::Result<#ident> {
                if self.memory_limit_bytes == Some(0) {
                    return Err(::monstermemory::Error::Configuration(
                        "memory_limit_bytes must be greater than zero".to_owned(),
                    ));
                }
                let _ = self.snapshot_warning_after;
                Ok(#ident::memory())
            }
        }

        impl #ident {
            /// Opens an empty, memory-only database with generated indexes.
            ///
            /// Starts at commit sequence zero. No files are read or written by this
            /// constructor, even when the database supports persistence methods.
            #[must_use]
            pub fn memory() -> Self {
                let core = ::monstermemory::__private::DatabaseCore::__memory(
                    #fingerprint,
                    vec![
                        #(::monstermemory::__private::__empty_table_root::<#record_types>()),*
                    ],
                );
                #validator_install
                Self {
                    #(#field_names: ::monstermemory::__private::Table::__new(
                        core.clone(), #table_ids,
                    ),)*
                }
            }

            /// Returns an advanced configuration builder. No settings are required.
            ///
            /// The defaults leave the memory budget unset and store a 30-second snapshot
            /// warning age. The alpha builder does not enforce either setting at runtime.
            #[must_use]
            pub fn builder() -> #builder_ident {
                #builder_ident {
                    memory_limit_bytes: None,
                    snapshot_warning_after: ::std::time::Duration::from_secs(30),
                }
            }

            /// Captures one database-wide immutable root.
            ///
            /// All snapshot tables observe one commit sequence. Later commits leave this
            /// view unchanged; drop it and its iterators to release retained history.
            #[must_use]
            pub fn snapshot(&self) -> #snapshot_ident {
                #snapshot_ident::__new(self.#first_field.__core().__snapshot())
            }

            /// Executes multiple typed operations and publishes them atomically.
            ///
            /// The closure runs once against staged writes based on one captured root. A
            /// successful nonempty commit publishes all affected tables in one root update.
            /// An empty transaction does not advance the sequence or invoke validation/WAL
            /// hooks. Errors do not publish the staged root; external closure side effects
            /// are outside this transaction.
            ///
            /// # Errors
            ///
            /// Propagates an error from the closure. Commit can return write conflicts,
            /// unique-index violations, configured validator errors, or commit-sequence
            /// exhaustion. A database opened for persistence can also return WAL encoding
            /// or I/O errors before publication. This method does not retry failures.
            pub fn write<R, F>(&self, operation: F) -> ::monstermemory::Result<R>
            where
                F: FnOnce(&mut #transaction_ident) -> ::monstermemory::Result<R>,
            {
                let core = self.#first_field.__core();
                let context = core.__transaction();
                let mut transaction = #transaction_ident {
                    #(#field_names: ::monstermemory::__private::WriteTable::__new(
                        context.clone(), #table_ids,
                    ),)*
                };
                let output = operation(&mut transaction)?;
                drop(transaction);
                #commit?;
                Ok(output)
            }

            /// Executes a closure again only when commit validation reports a write conflict.
            ///
            /// The closure can run more than once and therefore must not contain unguarded
            /// external side effects.
            /// Each retry captures a fresh base root and starts with an empty write buffer.
            /// A closure-returned error is propagated immediately, even if it is retryable;
            /// only retryable errors from commit cause another attempt.
            ///
            /// # Errors
            ///
            /// Returns the same closure and commit errors as `write`. A write conflict is
            /// returned when the attempt limit is exhausted. Nonretryable commit errors,
            /// such as uniqueness or persistence failures, stop immediately. A validator
            /// returning a retryable error participates in the same commit retry policy.
            pub fn retrying_write<R, F>(
                &self,
                policy: ::monstermemory::RetryPolicy,
                mut operation: F,
            ) -> ::monstermemory::Result<R>
            where
                F: FnMut(&mut #transaction_ident) -> ::monstermemory::Result<R>,
            {
                let core = self.#first_field.__core();
                for attempt in 0..policy.attempts() {
                    let context = core.__transaction();
                    let mut transaction = #transaction_ident {
                        #(#field_names: ::monstermemory::__private::WriteTable::__new(
                            context.clone(), #table_ids,
                        ),)*
                    };
                    let output = operation(&mut transaction)?;
                    drop(transaction);
                    match #commit_retry {
                        Ok(_) => return Ok(output),
                        Err(error) if error.is_retryable() && attempt + 1 < policy.attempts() => {}
                        Err(error) => return Err(error),
                    }
                }
                unreachable!("RetryPolicy always contains at least one attempt")
            }

            /// Builds a new database and publishes all loader operations in one commit.
            ///
            /// No partially populated database is returned if loading or validation fails.
            /// An empty loader returns an empty database at sequence zero. The result is
            /// memory-only; persistence support does not implicitly open files here.
            ///
            /// # Errors
            ///
            /// Propagates loader errors such as duplicate primary keys, and commit errors
            /// such as unique-index violations or rejection by the configured validator.
            pub fn bulk_load<F>(operation: F) -> ::monstermemory::Result<Self>
            where
                F: FnOnce(&mut #bulk_ident) -> ::monstermemory::Result<()>,
            {
                let database = Self::memory();
                let core = database.#first_field.__core();
                let context = core.__transaction();
                let mut loader = #bulk_ident {
                    #(#field_names: ::monstermemory::__private::WriteTable::__new(
                        context.clone(), #table_ids,
                    ),)*
                };
                operation(&mut loader)?;
                drop(loader);
                #commit?;
                Ok(database)
            }

            /// Returns current database-wide counters.
            ///
            /// Independent counters are sampled during the call; this is a monitoring view,
            /// not an atomic snapshot of all metrics at one commit boundary.
            #[must_use]
            pub fn metrics(&self) -> ::monstermemory::__private::MetricsSnapshot {
                self.#first_field.metrics()
            }

            /// Returns a deterministic fingerprint of this database's declaration.
            ///
            /// Hashes the database name and field/record type spellings. Record fields,
            /// secondary indexes, and serialization layout are not included. Equality is
            /// therefore insufficient to establish persisted-schema compatibility, and hash
            /// collisions are possible. No migration support is generated.
            #[must_use]
            pub const fn schema_fingerprint() -> u64 {
                #fingerprint
            }

            #persistence_methods
        }
    };
    Ok(expanded)
}

fn table_record_type(ty: &Type) -> syn::Result<Type> {
    let Type::Path(path) = ty else {
        return Err(Error::new_spanned(
            ty,
            "database fields must have type Table<Record>",
        ));
    };
    let Some(segment) = path.path.segments.last() else {
        return Err(Error::new_spanned(
            ty,
            "database fields must have type Table<Record>",
        ));
    };
    if segment.ident != "Table" {
        return Err(Error::new_spanned(
            ty,
            "database fields must have type Table<Record>",
        ));
    }
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return Err(Error::new_spanned(
            ty,
            "Table requires exactly one record type",
        ));
    };
    if arguments.args.len() != 1 {
        return Err(Error::new_spanned(
            ty,
            "Table requires exactly one record type",
        ));
    }
    let Some(GenericArgument::Type(record)) = arguments.args.first() else {
        return Err(Error::new_spanned(
            ty,
            "Table requires a record type argument",
        ));
    };
    Ok(record.clone())
}

fn parse_database_attributes(attributes: &[syn::Attribute]) -> syn::Result<DatabaseOptions> {
    let mut options = DatabaseOptions::default();
    for attribute in attributes {
        if !attribute.path().is_ident("database") {
            continue;
        }
        attribute.parse_nested_meta(|meta| {
            if meta.path.is_ident("validate") {
                if options.validator.is_some() {
                    return Err(meta.error("duplicate validate option"));
                }
                let value = meta.value()?.parse::<syn::LitStr>()?;
                options.validator = Some(value.parse()?);
                Ok(())
            } else if meta.path.is_ident("persistence") {
                if options.persistence {
                    return Err(meta.error("duplicate persistence option"));
                }
                options.persistence = true;
                Ok(())
            } else {
                Err(meta.error(
                    "unknown database option; supported options: persistence, validate = \"path\"",
                ))
            }
        })?;
    }
    Ok(options)
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}
