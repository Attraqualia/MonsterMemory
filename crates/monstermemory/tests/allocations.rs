#![allow(missing_docs)]

use monstermemory::prelude::*;

#[derive(Clone, Debug, Table)]
struct Row {
    #[primary_key]
    id: u64,
}

#[derive(Database)]
struct Db {
    rows: Table<Row>,
}

#[test]
fn warm_primary_reads_allocate_nothing() {
    let db = Db::memory();
    db.rows.insert(Row { id: 1 }).unwrap();
    let snapshot = db.snapshot();
    for _ in 0..16 {
        assert!(db.rows.get(&1).is_some());
        assert!(snapshot.rows.get(&1).is_some());
    }
    let live = allocation_counter::measure(|| {
        for _ in 0..1_000 {
            std::hint::black_box(db.rows.get(&1));
        }
    });
    let retained = allocation_counter::measure(|| {
        for _ in 0..1_000 {
            std::hint::black_box(snapshot.rows.get(&1));
        }
    });
    assert_eq!(
        live.count_total, 0,
        "live read unexpectedly allocated: {live:?}"
    );
    assert_eq!(
        retained.count_total, 0,
        "snapshot read unexpectedly allocated: {retained:?}"
    );
}
