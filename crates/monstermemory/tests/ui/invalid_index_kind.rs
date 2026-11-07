use monstermemory::prelude::*;

#[derive(Clone, Table)]
#[composite_index(name = "bad", fields(country, age), kind = "hash")]
struct InvalidKind {
    #[primary_key]
    id: u64,
    country: String,
    age: u16,
}

fn main() {}

