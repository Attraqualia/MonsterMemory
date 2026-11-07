use monstermemory::prelude::*;

#[derive(Clone, Table)]
struct DuplicateIndex {
    #[primary_key]
    id: u64,
    #[index(name = "same")]
    first: u64,
    #[index(name = "same")]
    second: u64,
}

fn main() {}

