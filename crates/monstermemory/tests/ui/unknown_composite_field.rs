use monstermemory::prelude::*;

#[derive(Clone, Table)]
#[composite_index(name = "bad", fields(country, missing), kind = "ordered")]
struct UnknownCompositeField {
    #[primary_key]
    id: u64,
    country: String,
}

fn main() {}

