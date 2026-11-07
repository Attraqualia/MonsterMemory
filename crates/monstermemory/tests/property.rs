#![allow(missing_docs)]

use monstermemory::prelude::*;
use proptest::prelude::*;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Table)]
struct Item {
    #[primary_key]
    id: u8,
    value: u16,
}

#[derive(Database)]
struct ItemDb {
    items: Table<Item>,
}

proptest! {
    #[test]
    fn random_operations_match_a_btree_reference_model(
        operations in prop::collection::vec((0_u8..4, any::<u8>(), any::<u16>()), 0..200)
    ) {
        let db = ItemDb::memory();
        let mut model = BTreeMap::new();
        for (operation, id, value) in operations {
            match operation {
                0 => {
                    let actual = db.items.insert(Item { id, value });
                    let expected = !model.contains_key(&id);
                    if expected { model.insert(id, value); }
                    prop_assert_eq!(actual.is_ok(), expected);
                }
                1 => {
                    db.items.upsert(Item { id, value }).unwrap();
                    model.insert(id, value);
                }
                2 => {
                    let actual = db.items.delete(&id);
                    let expected = model.remove(&id).is_some();
                    prop_assert_eq!(actual.is_ok(), expected);
                }
                _ => {
                    let actual = db.items.update(&id, |item| item.value = value);
                    let expected = model.get_mut(&id).map(|slot| *slot = value).is_some();
                    prop_assert_eq!(actual.is_ok(), expected);
                }
            }
            let actual: BTreeMap<_, _> = db.items.iter().map(|item| (item.id, item.value)).collect();
            prop_assert_eq!(&actual, &model);
        }
    }
}
