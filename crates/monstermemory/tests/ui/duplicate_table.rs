use monstermemory::prelude::*;

#[derive(Clone, Table)]
struct Row {
    #[primary_key]
    id: u64,
}

#[derive(Database)]
struct DuplicateTable {
    first: Table<Row>,
    second: Table<Row>,
}

fn main() {}

