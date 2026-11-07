use monstermemory::prelude::*;

#[derive(Clone, Table)]
struct Generic<T> {
    #[primary_key]
    id: u64,
    value: T,
}

fn main() {}

