use monstermemory::prelude::*;

#[derive(Clone, Table)]
struct UnknownIndexOption {
    #[primary_key]
    id: u64,
    #[index(magical)]
    value: u64,
}

fn main() {}

