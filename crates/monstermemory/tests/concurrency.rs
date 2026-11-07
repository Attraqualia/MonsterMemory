#![allow(missing_docs)]

use monstermemory::prelude::*;
use std::sync::{Arc, Barrier};

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

#[derive(Clone, Debug, Table)]
struct UniqueRow {
    #[primary_key]
    id: u64,
    #[index(unique)]
    name: String,
}

#[derive(Database)]
struct UniqueDb {
    rows: Table<UniqueRow>,
}

#[test]
fn first_committer_wins_for_the_same_key() {
    let db = CounterDb::memory();
    db.counters.insert(Counter { id: 1, value: 0 }).unwrap();
    let table = Arc::new(db.counters.clone());
    let barrier = Arc::new(Barrier::new(2));
    let workers: Vec<_> = (0..2)
        .map(|_| {
            let table = Arc::clone(&table);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                table.update(&1, |counter| {
                    counter.value += 1;
                    barrier.wait();
                })
            })
        })
        .collect();
    let results: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(Error::WriteConflict { .. })))
            .count(),
        1
    );
    assert_eq!(table.get(&1).unwrap().value, 1);
}

#[test]
fn readers_keep_old_roots_alive_while_writers_publish() {
    let db = CounterDb::memory();
    db.counters.insert(Counter { id: 1, value: 0 }).unwrap();
    let snapshot = db.snapshot();
    let table = Arc::new(db.counters.clone());
    let readers: Vec<_> = (0..4)
        .map(|_| {
            let table = Arc::clone(&table);
            std::thread::spawn(move || {
                for _ in 0..5_000 {
                    let value = table.get(&1).unwrap().value;
                    assert!(value <= 100);
                }
            })
        })
        .collect();
    for value in 1..=100 {
        table.update(&1, |counter| counter.value = value).unwrap();
    }
    for reader in readers {
        reader.join().unwrap();
    }
    assert_eq!(snapshot.counters.get(&1).unwrap().value, 0);
    assert_eq!(table.get(&1).unwrap().value, 100);
    assert!(db.metrics().active_snapshots >= 1);
}

#[test]
fn concurrent_unique_inserts_are_revalidated_against_the_current_root() {
    let db = Arc::new(UniqueDb::memory());
    let barrier = Arc::new(Barrier::new(2));
    let workers: Vec<_> = (1..=2)
        .map(|id| {
            let db = Arc::clone(&db);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                db.write(|tx| {
                    tx.rows.insert(UniqueRow {
                        id,
                        name: "same".into(),
                    })?;
                    barrier.wait();
                    Ok(())
                })
            })
        })
        .collect();
    let results: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(Error::UniqueIndexViolation { .. })))
            .count(),
        1
    );
    assert_eq!(db.rows.len(), 1);
}
