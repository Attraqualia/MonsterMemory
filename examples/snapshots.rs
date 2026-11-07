#![allow(missing_docs)]

use monstermemory::prelude::*;

#[derive(Clone, Debug, Table)]
struct Counter {
    #[primary_key]
    id: u64,
    value: u64,
}

#[derive(Database)]
struct CounterDb {
    counters: Table<Counter>,
}

fn main() -> Result<()> {
    let db = CounterDb::memory();
    db.counters.insert(Counter { id: 1, value: 1 })?;
    let before = db.snapshot();
    db.counters.update(&1, |counter| counter.value = 2)?;
    assert_eq!(before.counters.get(&1).unwrap().value, 1);
    assert_eq!(db.counters.get(&1).unwrap().value, 2);
    Ok(())
}
