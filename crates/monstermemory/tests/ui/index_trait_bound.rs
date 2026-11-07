use monstermemory::prelude::*;

#[derive(Clone)]
struct NotOrdered;

#[derive(Clone, Table)]
struct MissingIndexBound {
    #[primary_key]
    id: u64,
    #[index(range)]
    value: NotOrdered,
}

fn main() {}

