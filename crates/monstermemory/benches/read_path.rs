#![allow(missing_docs)]

use monstermemory::prelude::*;
use std::{
    collections::HashMap,
    hint::black_box,
    sync::RwLock,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Table)]
struct BenchRow {
    #[primary_key]
    id: u64,
    value: u64,
}

#[derive(Database)]
struct BenchDb {
    rows: Table<BenchRow>,
}

fn measure(mut operation: impl FnMut(), iterations: u32) -> Duration {
    let start = Instant::now();
    for _ in 0..iterations {
        operation();
    }
    start.elapsed()
}

fn per_operation(duration: Duration, iterations: u32) -> f64 {
    duration.as_secs_f64() * 1_000_000_000.0 / f64::from(iterations)
}

fn main() {
    let count = std::env::var("MONSTERMEMORY_BENCH_ROWS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(100_000_u64);
    let iterations = std::env::var("MONSTERMEMORY_BENCH_ITERS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(2_000_000_u32);
    let map: HashMap<_, _> = (0..count).map(|id| (id, id * 2)).collect();
    let locked = RwLock::new(map.clone());
    let db = BenchDb::bulk_load(|loader| {
        loader
            .rows
            .extend((0..count).map(|id| BenchRow { id, value: id * 2 }))?;
        Ok(())
    })
    .unwrap();
    let snapshot = db.snapshot();
    let mut cursor = 0_u64;
    let hash_map = measure(
        || {
            cursor = cursor.wrapping_add(0x9e37_79b9_7f4a_7c15) % count;
            black_box(map.get(&cursor));
        },
        iterations,
    );
    let rwlock = measure(
        || {
            cursor = cursor.wrapping_add(0x9e37_79b9_7f4a_7c15) % count;
            black_box(locked.read().unwrap().get(&cursor).copied());
        },
        iterations,
    );
    let live = measure(
        || {
            cursor = cursor.wrapping_add(0x9e37_79b9_7f4a_7c15) % count;
            black_box(db.rows.get(&cursor).map(|row| row.value));
        },
        iterations,
    );
    let snap = measure(
        || {
            cursor = cursor.wrapping_add(0x9e37_79b9_7f4a_7c15) % count;
            black_box(snapshot.rows.get(&cursor).map(|row| row.value));
        },
        iterations,
    );
    println!(
        "dataset_rows,iterations,hashmap_ns,rwlock_hashmap_ns,monstermemory_live_ns,monstermemory_snapshot_ns"
    );
    println!(
        "{count},{iterations},{:.2},{:.2},{:.2},{:.2}",
        per_operation(hash_map, iterations),
        per_operation(rwlock, iterations),
        per_operation(live, iterations),
        per_operation(snap, iterations),
    );
}
