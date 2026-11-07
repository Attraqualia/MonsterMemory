//! Reduced concurrency models for publication and first-committer-wins.

#![allow(missing_docs)]

use loom::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    thread,
};

#[test]
fn reader_observes_one_complete_immutable_root() {
    loom::model(|| {
        let published = Arc::new(AtomicU64::new(0));
        let reader_root = Arc::clone(&published);
        let writer_root = Arc::clone(&published);
        let reader = thread::spawn(move || {
            let observed = reader_root.load(Ordering::Acquire);
            assert!(observed == 0 || observed == 1);
            observed
        });
        let writer = thread::spawn(move || writer_root.store(1, Ordering::Release));
        writer.join().unwrap();
        let observed = reader.join().unwrap();
        assert!(observed == 0 || observed == 1);
    });
}

#[test]
fn only_one_writer_validates_the_same_base_version() {
    loom::model(|| {
        let version = Arc::new(AtomicU64::new(0));
        let writers: Vec<_> = (0..2)
            .map(|_| {
                let version = Arc::clone(&version);
                thread::spawn(move || {
                    version
                        .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
                        .is_ok()
                })
            })
            .collect();
        let commits = writers
            .into_iter()
            .map(|writer| writer.join().unwrap())
            .filter(|committed| *committed)
            .count();
        assert_eq!(commits, 1);
    });
}
