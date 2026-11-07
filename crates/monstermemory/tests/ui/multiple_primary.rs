use monstermemory::prelude::*;

#[derive(Clone, Table)]
struct MultiplePrimary {
    #[primary_key]
    first: u64,
    #[primary_key]
    second: u64,
}

fn main() {}

