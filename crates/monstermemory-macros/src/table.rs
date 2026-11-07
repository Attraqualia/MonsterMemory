use proc_macro2::TokenStream;
use quote::{format_ident, quote, quote_spanned};
use std::collections::{HashMap, HashSet};
use syn::{
    Attribute, Data, DeriveInput, Error, Field, Fields, Ident, LitStr, Type, spanned::Spanned as _,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Unique,
    Exact,
    Ordered,
    CompositeOrdered,
}

struct IndexSpec {
    name: String,
    kind: Kind,
    fields: Vec<(Ident, Type)>,
    span: proc_macro2::Span,
}

struct CompositeSpec {
    name: String,
    fields: Vec<Ident>,
    span: proc_macro2::Span,
}

#[allow(
    clippy::too_many_lines,
    reason = "schema validation and emitted impls share parsed spans"
)]
pub(crate) fn expand(input: TokenStream) -> syn::Result<TokenStream> {
    let input: DeriveInput = syn::parse2(input)?;
    if !input.generics.params.is_empty() {
        return Err(Error::new_spanned(
            &input.generics,
            "MonsterMemory tables cannot be generic or contain lifetimes; define a concrete record type",
        ));
    }
    let Data::Struct(data) = &input.data else {
        return Err(Error::new_spanned(
            &input.ident,
            "Table can only be derived for a struct",
        ));
    };
    let Fields::Named(fields) = &data.fields else {
        return Err(Error::new_spanned(
            &data.fields,
            "Table requires named fields so generated diagnostics and indexes remain stable",
        ));
    };

    let table_name = parse_table_name(&input.attrs, &input.ident)?;
    let mut primary: Option<&Field> = None;
    let mut indexes = Vec::new();
    let mut field_map = HashMap::new();
    for field in &fields.named {
        let ident = field.ident.as_ref().expect("named field");
        field_map.insert(ident.to_string(), field);
        for attribute in &field.attrs {
            if attribute.path().is_ident("primary_key") {
                if !matches!(attribute.meta, syn::Meta::Path(_)) {
                    return Err(Error::new_spanned(
                        attribute,
                        "#[primary_key] does not accept options",
                    ));
                }
                if let Some(previous) = primary {
                    let mut error =
                        Error::new_spanned(field, "a table must have exactly one primary key");
                    error.combine(Error::new_spanned(
                        previous,
                        "the first primary key is here",
                    ));
                    return Err(error);
                }
                primary = Some(field);
            } else if attribute.path().is_ident("index") {
                indexes.push(parse_field_index(attribute, field)?);
            }
        }
    }
    let Some(primary) = primary else {
        return Err(Error::new_spanned(
            &input.ident,
            "missing primary key; add #[primary_key] to exactly one field",
        ));
    };

    for attribute in &input.attrs {
        if attribute.path().is_ident("composite_index") {
            let composite = parse_composite(attribute)?;
            let mut resolved = Vec::new();
            for field_name in &composite.fields {
                let Some(field) = field_map.get(&field_name.to_string()) else {
                    return Err(Error::new_spanned(
                        field_name,
                        format!(
                            "composite index `{}` refers to unknown field `{field_name}`",
                            composite.name
                        ),
                    ));
                };
                resolved.push((field_name.clone(), field.ty.clone()));
            }
            indexes.push(IndexSpec {
                name: composite.name,
                kind: Kind::CompositeOrdered,
                fields: resolved,
                span: composite.span,
            });
        }
    }
    validate_indexes(&indexes)?;

    let ident = &input.ident;
    let visibility = &input.vis;
    let primary_ident = primary.ident.as_ref().expect("named field");
    let primary_ty = &primary.ty;
    let fingerprint_material = format!(
        "{}:{}:{}:{:?}",
        ident,
        table_name,
        quote!(#primary_ty),
        indexes
            .iter()
            .map(|index| (&index.name, index.kind))
            .collect::<Vec<_>>()
    );
    let fingerprint = fnv1a(fingerprint_material.as_bytes());

    let definitions = indexes.iter().map(|index| {
        let name = &index.name;
        let kind = match index.kind {
            Kind::Unique => quote!(::monstermemory::__private::IndexKind::Unique),
            Kind::Exact => quote!(::monstermemory::__private::IndexKind::Exact),
            Kind::Ordered => quote!(::monstermemory::__private::IndexKind::Ordered),
            Kind::CompositeOrdered => {
                quote!(::monstermemory::__private::IndexKind::CompositeOrdered)
            }
        };
        let fields = index.fields.iter().map(|(field, _)| field.to_string());
        quote! {
            ::monstermemory::__private::IndexDefinition {
                name: #name,
                kind: #kind,
                fields: &[#(#fields),*],
            }
        }
    });

    // Metadata, extractors, and emitted queries must share this order: the engine addresses
    // secondary indexes by position rather than by their diagnostic names.
    let extractors = indexes.iter().map(|index| {
        let fields = index.fields.iter().map(|(field, _)| {
            quote_spanned! {field.span()=>
                ::monstermemory::__private::KeyPart::from_value(&self.#field)
            }
        });
        quote! {
            ::monstermemory::__private::IndexKey::from_parts([#(#fields),*])
        }
    });

    let query_trait = format_ident!("{}TableQueries", ident);
    let (declarations, implementations) = query_methods(ident, &indexes);

    Ok(quote! {
        impl ::monstermemory::__private::TableSchema for #ident {
            type PrimaryKey = #primary_ty;

            fn primary_key(&self) -> Self::PrimaryKey {
                self.#primary_ident.clone()
            }

            fn index_keys(&self) -> ::std::vec::Vec<::monstermemory::__private::IndexKey> {
                vec![#(#extractors),*]
            }

            fn metadata() -> &'static ::monstermemory::__private::TableMetadata {
                static INDEXES: &[::monstermemory::__private::IndexDefinition] = &[#(#definitions),*];
                static METADATA: ::monstermemory::__private::TableMetadata =
                    ::monstermemory::__private::TableMetadata {
                        name: #table_name,
                        rust_type: stringify!(#ident),
                        primary_key: stringify!(#primary_ident),
                        indexes: INDEXES,
                        fingerprint: #fingerprint,
                    };
                &METADATA
            }
        }

        #[doc = concat!("Generated typed index queries for [`", stringify!(#ident), "`].")]
        ///
        /// Import this trait to call its methods on live and snapshot table handles. Lazy
        /// queries retain their captured database root and do not observe later commits.
        #visibility trait #query_trait {
            #(#declarations)*
        }

        impl #query_trait for ::monstermemory::__private::Table<#ident> {
            #(#implementations)*
        }

        impl #query_trait for ::monstermemory::__private::SnapshotTable<#ident> {
            #(#implementations)*
        }
    })
}

fn parse_table_name(attributes: &[Attribute], ident: &Ident) -> syn::Result<String> {
    let mut name = None;
    for attribute in attributes {
        if !attribute.path().is_ident("table") {
            continue;
        }
        attribute.parse_nested_meta(|meta| {
            if meta.path.is_ident("name") {
                if name.is_some() {
                    return Err(meta.error("duplicate `name` option"));
                }
                name = Some(meta.value()?.parse::<LitStr>()?.value());
                Ok(())
            } else {
                Err(meta.error("unknown table option; supported option: name = \"...\""))
            }
        })?;
    }
    Ok(name.unwrap_or_else(|| to_snake_case(&ident.to_string())))
}

fn parse_field_index(attribute: &Attribute, field: &Field) -> syn::Result<IndexSpec> {
    let ident = field.ident.as_ref().expect("named field");
    let mut name = ident.to_string();
    let mut kind = Kind::Exact;
    let mut kind_seen = false;
    if matches!(attribute.meta, syn::Meta::List(_)) {
        attribute.parse_nested_meta(|meta| {
            if meta.path.is_ident("unique") {
                if kind_seen {
                    return Err(meta.error("an index can have only one kind"));
                }
                kind = Kind::Unique;
                kind_seen = true;
                Ok(())
            } else if meta.path.is_ident("range") || meta.path.is_ident("ordered") {
                if kind_seen {
                    return Err(meta.error("an index can have only one kind"));
                }
                kind = Kind::Ordered;
                kind_seen = true;
                Ok(())
            } else if meta.path.is_ident("name") {
                name = meta.value()?.parse::<LitStr>()?.value();
                Ok(())
            } else {
                Err(meta
                    .error("unknown index option; use unique, range, ordered, or name = \"...\""))
            }
        })?;
    }
    Ok(IndexSpec {
        name,
        kind,
        fields: vec![(ident.clone(), field.ty.clone())],
        span: attribute.span(),
    })
}

fn parse_composite(attribute: &Attribute) -> syn::Result<CompositeSpec> {
    let mut name = None;
    let mut fields = Vec::new();
    let mut kind = None;
    attribute.parse_nested_meta(|meta| {
        if meta.path.is_ident("name") {
            name = Some(meta.value()?.parse::<LitStr>()?.value());
            Ok(())
        } else if meta.path.is_ident("fields") {
            meta.parse_nested_meta(|field| {
                let Some(ident) = field.path.get_ident() else {
                    return Err(field.error("composite fields must be plain field names"));
                };
                fields.push(ident.clone());
                Ok(())
            })
        } else if meta.path.is_ident("kind") {
            kind = Some(meta.value()?.parse::<LitStr>()?.value());
            Ok(())
        } else {
            Err(meta.error("unknown composite_index option; use name, fields(...), or kind"))
        }
    })?;
    let name = name
        .ok_or_else(|| Error::new_spanned(attribute, "composite index requires name = \"...\""))?;
    if fields.len() < 2 {
        return Err(Error::new_spanned(
            attribute,
            "composite index requires at least two fields",
        ));
    }
    if kind.as_deref() != Some("ordered") {
        return Err(Error::new_spanned(
            attribute,
            "invalid composite index kind; the supported kind is \"ordered\"",
        ));
    }
    let mut seen = HashSet::new();
    for field in &fields {
        if !seen.insert(field.to_string()) {
            return Err(Error::new_spanned(
                field,
                "duplicate field in composite index",
            ));
        }
    }
    Ok(CompositeSpec {
        name,
        fields,
        span: attribute.span(),
    })
}

fn validate_indexes(indexes: &[IndexSpec]) -> syn::Result<()> {
    let reserved = [
        "get",
        "get_cloned",
        "insert",
        "insert_many",
        "upsert",
        "upsert_many",
        "update",
        "delete",
        "delete_many",
        "iter",
        "len",
        "is_empty",
        "metrics",
    ];
    let mut names = HashMap::new();
    let mut methods = HashSet::new();
    for index in indexes {
        let _: Ident = syn::parse_str(&index.name)
            .map_err(|_| Error::new(index.span, "index name must be a valid Rust identifier"))?;
        if let Some(previous) = names.insert(index.name.clone(), index.span) {
            let mut error =
                Error::new(index.span, format!("duplicate index name `{}`", index.name));
            error.combine(Error::new(
                previous,
                "the first index with this name is here",
            ));
            return Err(error);
        }
        let method = if index.kind == Kind::Ordered {
            format!("{}_range", index.name)
        } else {
            format!("by_{}", index.name)
        };
        if reserved.contains(&method.as_str()) || !methods.insert(method.clone()) {
            return Err(Error::new(
                index.span,
                format!("generated query method `{method}` collides with another table method"),
            ));
        }
    }
    Ok(())
}

#[allow(
    clippy::too_many_lines,
    reason = "each index kind emits one self-contained typed method"
)]
fn query_methods(record: &Ident, indexes: &[IndexSpec]) -> (Vec<TokenStream>, Vec<TokenStream>) {
    let mut declarations = Vec::new();
    let mut implementations = Vec::new();
    for (index_id, index) in indexes.iter().enumerate() {
        let index_id = syn::Index::from(index_id);
        match index.kind {
            Kind::Unique | Kind::Exact => {
                let method = format_ident!("by_{}", index.name);
                let ty = &index.fields[0].1;
                let output = if index.kind == Kind::Unique {
                    quote!(::core::option::Option<::monstermemory::__private::RecordRef<#record>>)
                } else {
                    quote!(::monstermemory::__private::IndexQuery<#record>)
                };
                let documentation = if index.kind == Kind::Unique {
                    "Returns the immutable record matching this unique index, or `None`.\n\n\
                     A live table reads one current root; a snapshot table uses its retained root. \
                     The returned handle keeps that record version alive independently of later \
                     updates. Borrowed lookup values must encode the same key as the stored field."
                } else {
                    "Lazily iterates all records equal to this index key in logical-row order.\n\n\
                     An absent key yields an empty iterator. The query retains one captured root \
                     until dropped and never observes later commits. Borrowed lookup values must \
                     encode the same key as the stored field."
                };
                declarations.push(quote! {
                    #[doc = #documentation]
                    fn #method<Q>(&self, key: &Q) -> #output
                    where
                        Q: ?Sized + ::monstermemory::__private::Lookup<#ty>;
                });
                let body = if index.kind == Kind::Unique {
                    quote!(self.__unique_index(#index_id, &encoded))
                } else {
                    quote! {
                        self.__index_query(
                            #index_id,
                            ::core::ops::Bound::Included(encoded.clone()),
                            ::core::ops::Bound::Included(encoded),
                            None,
                        )
                    }
                };
                implementations.push(quote! {
                    fn #method<Q>(&self, key: &Q) -> #output
                    where
                        Q: ?Sized + ::monstermemory::__private::Lookup<#ty>,
                    {
                        let encoded = ::monstermemory::__private::IndexKey::single(
                            ::monstermemory::__private::KeyPart::from_lookup::<#ty, Q>(key),
                        );
                        #body
                    }
                });
            }
            Kind::Ordered => {
                let method = format_ident!("{}_range", index.name);
                let ty = &index.fields[0].1;
                declarations.push(quote! {
                    #[doc = concat!("Scans the generated `", stringify!(#method), "` index lazily.")]
                    ///
                    /// Inclusive, exclusive, and unbounded endpoints follow `RangeBounds`.
                    /// Rows are ordered by encoded index value, then logical-row order for ties.
                    /// No matches yields an empty iterator. The query keeps one captured root
                    /// alive until dropped and does not observe later commits.
                    fn #method<R>(&self, range: R) -> ::monstermemory::__private::IndexQuery<#record>
                    where
                        R: ::core::ops::RangeBounds<#ty>;
                });
                implementations.push(quote! {
                    fn #method<R>(&self, range: R) -> ::monstermemory::__private::IndexQuery<#record>
                    where
                        R: ::core::ops::RangeBounds<#ty>,
                    {
                        let lower = match ::core::ops::RangeBounds::start_bound(&range) {
                            ::core::ops::Bound::Included(value) => ::core::ops::Bound::Included(
                                ::monstermemory::__private::IndexKey::single(
                                    ::monstermemory::__private::KeyPart::from_value(value),
                                ),
                            ),
                            ::core::ops::Bound::Excluded(value) => ::core::ops::Bound::Excluded(
                                ::monstermemory::__private::IndexKey::single(
                                    ::monstermemory::__private::KeyPart::from_value(value),
                                ),
                            ),
                            ::core::ops::Bound::Unbounded => ::core::ops::Bound::Unbounded,
                        };
                        let upper = match ::core::ops::RangeBounds::end_bound(&range) {
                            ::core::ops::Bound::Included(value) => ::core::ops::Bound::Included(
                                ::monstermemory::__private::IndexKey::single(
                                    ::monstermemory::__private::KeyPart::from_value(value),
                                ),
                            ),
                            ::core::ops::Bound::Excluded(value) => ::core::ops::Bound::Excluded(
                                ::monstermemory::__private::IndexKey::single(
                                    ::monstermemory::__private::KeyPart::from_value(value),
                                ),
                            ),
                            ::core::ops::Bound::Unbounded => ::core::ops::Bound::Unbounded,
                        };
                        self.__index_query(#index_id, lower, upper, None)
                    }
                });
            }
            Kind::CompositeOrdered => {
                let method = format_ident!("by_{}", index.name);
                let prefix_fields = &index.fields[..index.fields.len() - 1];
                let suffix_ty = &index.fields.last().expect("two fields").1;
                let query_types: Vec<Ident> = (0..prefix_fields.len())
                    .map(|position| format_ident!("Q{position}"))
                    .collect();
                let query_values: Vec<Ident> = (0..prefix_fields.len())
                    .map(|position| format_ident!("key{position}"))
                    .collect();
                let stored_types: Vec<&Type> = prefix_fields.iter().map(|(_, ty)| ty).collect();
                let where_lookups = query_types
                    .iter()
                    .zip(&stored_types)
                    .map(|(query, stored)| quote!(#query: ?Sized + ::monstermemory::__private::Lookup<#stored>));
                let prefix_parts = query_values
                    .iter()
                    .zip(query_types.iter().zip(&stored_types))
                    .map(|(value, (query, stored))| {
                        quote!(::monstermemory::__private::KeyPart::from_lookup::<#stored, #query>(#value))
                    });
                declarations.push(quote! {
                    #[doc = concat!("Scans the generated composite `", stringify!(#method), "` index.")]
                    ///
                    /// Pass a tuple containing borrowed exact values for every prefix field,
                    /// followed by a `RangeBounds` value for the last field, all in declaration
                    /// order. Prefix fields compare whole values. Range endpoints preserve
                    /// inclusion, exclusion, and unboundedness. Results sort by encoded index
                    /// key, then logical-row order; no matches yields an empty iterator.
                    /// The query retains one root until dropped and never observes later commits.
                    fn #method<#(#query_types,)* R>(
                        &self,
                        query: (#(&#query_types,)* R),
                    ) -> ::monstermemory::__private::IndexQuery<#record>
                    where
                        #(#where_lookups,)*
                        R: ::core::ops::RangeBounds<#suffix_ty>;
                });
                let where_lookups = query_types
                    .iter()
                    .zip(&stored_types)
                    .map(|(query, stored)| quote!(#query: ?Sized + ::monstermemory::__private::Lookup<#stored>));
                implementations.push(quote! {
                    fn #method<#(#query_types,)* R>(
                        &self,
                        query: (#(&#query_types,)* R),
                    ) -> ::monstermemory::__private::IndexQuery<#record>
                    where
                        #(#where_lookups,)*
                        R: ::core::ops::RangeBounds<#suffix_ty>,
                    {
                        let (#(#query_values,)* range) = query;
                        let prefix = ::monstermemory::__private::IndexKey::from_parts([
                            #(#prefix_parts),*
                        ]);
                        let lower = match ::core::ops::RangeBounds::start_bound(&range) {
                            ::core::ops::Bound::Included(value) => ::core::ops::Bound::Included(
                                prefix.clone().with_part(
                                    ::monstermemory::__private::KeyPart::from_value(value),
                                ),
                            ),
                            ::core::ops::Bound::Excluded(value) => ::core::ops::Bound::Excluded(
                                prefix.clone().with_part(
                                    ::monstermemory::__private::KeyPart::from_value(value),
                                ),
                            ),
                            ::core::ops::Bound::Unbounded => ::core::ops::Bound::Included(prefix.clone()),
                        };
                        let upper = match ::core::ops::RangeBounds::end_bound(&range) {
                            ::core::ops::Bound::Included(value) => ::core::ops::Bound::Included(
                                prefix.clone().with_part(
                                    ::monstermemory::__private::KeyPart::from_value(value),
                                ),
                            ),
                            ::core::ops::Bound::Excluded(value) => ::core::ops::Bound::Excluded(
                                prefix.clone().with_part(
                                    ::monstermemory::__private::KeyPart::from_value(value),
                                ),
                            ),
                            ::core::ops::Bound::Unbounded => ::core::ops::Bound::Unbounded,
                        };
                        self.__index_query(#index_id, lower, upper, Some(prefix))
                    }
                });
            }
        }
    }
    (declarations, implementations)
}

fn to_snake_case(value: &str) -> String {
    let mut output = String::new();
    for (index, character) in value.chars().enumerate() {
        if character.is_uppercase() {
            if index != 0 {
                output.push('_');
            }
            output.extend(character.to_lowercase());
        } else {
            output.push(character);
        }
    }
    output
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}
