#![allow(missing_docs)]

use monstermemory::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, Table)]
struct Setting {
    #[primary_key]
    id: u64,
    value: String,
}

#[derive(Database)]
#[database(persistence)]
struct SettingsDb {
    settings: Table<Setting>,
}

fn main() -> Result<()> {
    let path = std::env::temp_dir().join("monstermemory-persistence-example.bin");
    let db = SettingsDb::memory();
    db.settings.insert(Setting {
        id: 1,
        value: "safe".into(),
    })?;
    db.checkpoint(&path)?;
    let reopened = SettingsDb::open(&path)?;
    assert_eq!(reopened.settings.get(&1).unwrap().value, "safe");
    std::fs::remove_file(path)?;
    Ok(())
}
