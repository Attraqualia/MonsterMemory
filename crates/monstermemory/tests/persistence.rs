#![allow(missing_docs)]
#![cfg(feature = "persistence")]

use monstermemory::prelude::*;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

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

fn path(name: &str) -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("monstermemory-{name}-{nonce}.bin"))
}

#[test]
fn generated_checkpoint_reopens_and_can_be_replaced() -> Result<()> {
    let path = path("facade");
    let db = SettingsDb::open(&path)?;
    db.settings.insert(Setting {
        id: 1,
        value: "first".into(),
    })?;
    db.checkpoint(&path)?;
    db.settings
        .update(&1, |setting| setting.value = "second".into())?;
    db.checkpoint(&path)?;
    let reopened = SettingsDb::open(&path)?;
    assert_eq!(reopened.settings.get(&1).unwrap().value, "second");
    std::fs::remove_file(path)?;
    Ok(())
}

#[test]
fn open_recovers_the_last_committed_wal_without_a_checkpoint() -> Result<()> {
    let path = path("recovery");
    {
        let db = SettingsDb::open(&path)?;
        db.settings.insert(Setting {
            id: 7,
            value: "from-wal".into(),
        })?;
    }
    let recovered = SettingsDb::open(&path)?;
    assert_eq!(recovered.settings.get(&7).unwrap().value, "from-wal");
    let wal = monstermemory::persistence::wal_path(&path);
    recovered.checkpoint(&path)?;
    assert!(!wal.exists());
    std::fs::remove_file(path)?;
    Ok(())
}
