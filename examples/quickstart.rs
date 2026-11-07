#![allow(missing_docs)]

use monstermemory::prelude::*;

#[derive(Clone, Debug, Table)]
#[table(name = "users")]
#[composite_index(name = "country_age", fields(country, age), kind = "ordered")]
pub struct User {
    #[primary_key]
    pub id: u64,
    #[index(unique)]
    pub email: String,
    #[index]
    pub country: String,
    #[index(range)]
    pub age: u16,
}

#[derive(Database)]
pub struct AppDb {
    pub users: Table<User>,
}

fn main() -> monstermemory::Result<()> {
    let db = AppDb::memory();

    db.users.insert(User {
        id: 1,
        email: "alice@example.com".into(),
        country: "JP".into(),
        age: 28,
    })?;

    db.users.update(&1, |user| user.age += 1)?;

    let alice = db.users.get(&1).expect("user exists");
    assert_eq!(alice.age, 29);
    assert_eq!(db.users.by_email("alice@example.com").unwrap().id, alice.id);
    Ok(())
}
