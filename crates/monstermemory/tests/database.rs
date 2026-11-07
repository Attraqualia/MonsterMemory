#![allow(missing_docs)]

use monstermemory::prelude::*;

#[derive(Clone, Debug, Eq, PartialEq, Table)]
#[table(name = "users")]
#[composite_index(name = "country_age", fields(country, age), kind = "ordered")]
struct User {
    #[primary_key]
    id: u64,
    #[index(unique)]
    email: String,
    #[index]
    country: String,
    #[index(range)]
    age: u16,
    display_name: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Table)]
struct Audit {
    #[primary_key]
    id: u64,
    user_id: u64,
}

#[derive(Database)]
struct AppDb {
    users: Table<User>,
    audits: Table<Audit>,
}

fn user(id: u64, email: &str, country: &str, age: u16) -> User {
    User {
        id,
        email: email.to_owned(),
        country: country.to_owned(),
        age,
        display_name: format!("user-{id}"),
    }
}

#[test]
fn crud_and_all_index_kinds_stay_consistent() -> Result<()> {
    let db = AppDb::memory();
    db.users.insert_many([
        user(1, "a@example.com", "JP", 20),
        user(2, "b@example.com", "JP", 30),
        user(3, "c@example.com", "US", 40),
    ])?;

    assert_eq!(db.users.by_email("b@example.com").unwrap().id, 2);
    assert_eq!(
        db.users
            .by_country("JP")
            .map(|row| row.id)
            .collect::<Vec<_>>(),
        [1, 2]
    );
    assert_eq!(
        db.users
            .age_range(20..40)
            .map(|row| row.id)
            .collect::<Vec<_>>(),
        [1, 2]
    );
    assert_eq!(
        db.users
            .by_country_age(("JP", 25..))
            .map(|row| row.id)
            .collect::<Vec<_>>(),
        [2]
    );

    db.users.update(&2, |row| {
        row.email = "new@example.com".into();
        row.country = "US".into();
    })?;
    assert!(db.users.by_email("b@example.com").is_none());
    assert_eq!(db.users.by_email("new@example.com").unwrap().id, 2);
    assert_eq!(db.users.by_country("JP").count(), 1);
    assert_eq!(db.users.by_country("US").count(), 2);

    db.users.delete(&1)?;
    assert!(db.users.get(&1).is_none());
    db.users.upsert(user(1, "restored@example.com", "JP", 21))?;
    assert_eq!(db.users.get_cloned(&1).unwrap().age, 21);
    Ok(())
}

#[test]
fn snapshot_is_repeatable_and_database_wide() -> Result<()> {
    let db = AppDb::memory();
    db.write(|tx| {
        tx.users.insert(user(1, "a@example.com", "JP", 20))?;
        tx.audits.insert(Audit { id: 1, user_id: 1 })?;
        Ok(())
    })?;
    let snapshot = db.snapshot();
    let sequence = snapshot.sequence();
    db.write(|tx| {
        tx.users.update(&1, |row| row.age = 99)?;
        tx.audits.delete(&1)?;
        Ok(())
    })?;

    assert_eq!(snapshot.users.get(&1).unwrap().age, 20);
    assert_eq!(snapshot.audits.get(&1).unwrap().user_id, 1);
    assert!(db.snapshot().sequence() > sequence);
    assert_eq!(db.users.get(&1).unwrap().age, 99);
    assert!(db.audits.get(&1).is_none());
    Ok(())
}

#[test]
fn failed_multi_table_transaction_publishes_nothing() -> Result<()> {
    let db = AppDb::memory();
    db.users.insert(user(1, "taken@example.com", "JP", 20))?;
    let sequence = db.snapshot().sequence();
    let error = db
        .write(|tx| {
            tx.audits.insert(Audit { id: 1, user_id: 1 })?;
            tx.users.insert(user(2, "taken@example.com", "US", 30))?;
            Ok(())
        })
        .unwrap_err();
    assert!(matches!(error, Error::UniqueIndexViolation { .. }));
    assert!(db.audits.is_empty());
    assert_eq!(db.snapshot().sequence(), sequence);
    Ok(())
}

#[test]
fn primary_key_changes_are_rejected_before_commit() -> Result<()> {
    let db = AppDb::memory();
    db.users.insert(user(1, "a@example.com", "JP", 20))?;
    let error = db.users.update(&1, |row| row.id = 2).unwrap_err();
    assert!(matches!(error, Error::PrimaryKeyChanged { .. }));
    assert!(db.users.get(&1).is_some());
    assert!(db.users.get(&2).is_none());
    Ok(())
}

#[test]
fn unchanged_indexes_do_not_receive_mutations() -> Result<()> {
    let db = AppDb::memory();
    db.users.insert(user(1, "a@example.com", "JP", 20))?;
    let before = db.metrics().changed_index_paths;
    db.users
        .update(&1, |row| row.display_name = "Alice".into())?;
    assert_eq!(db.metrics().changed_index_paths, before);
    db.users.update(&1, |row| row.age += 1)?;
    // age and country_age are the only affected generated indexes.
    assert_eq!(db.metrics().changed_index_paths, before + 4);
    Ok(())
}

#[test]
fn bulk_load_is_atomic_and_deterministic() -> Result<()> {
    let db = AppDb::bulk_load(|loader| {
        loader.users.extend((0..100).rev().map(|id| {
            user(
                id,
                &format!("{id}@example.com"),
                "JP",
                u16::try_from(id).expect("test range fits u16"),
            )
        }))?;
        Ok(())
    })?;
    assert_eq!(db.users.len(), 100);
    assert_eq!(db.users.age_range(..).count(), 100);
    assert_eq!(db.snapshot().sequence(), 1);
    Ok(())
}
