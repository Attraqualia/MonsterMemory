#![allow(missing_docs)]

use monstermemory::prelude::*;

#[derive(Clone, Debug, Table)]
#[composite_index(name = "region_score", fields(region, score), kind = "ordered")]
struct Player {
    #[primary_key]
    id: u64,
    #[index(unique)]
    handle: String,
    #[index]
    region: String,
    #[index(range)]
    score: u32,
}

#[derive(Database)]
struct GameDb {
    players: Table<Player>,
}

fn main() -> Result<()> {
    let db = GameDb::bulk_load(|loader| {
        loader.players.extend([
            Player {
                id: 1,
                handle: "moth".into(),
                region: "JP".into(),
                score: 40,
            },
            Player {
                id: 2,
                handle: "slime".into(),
                region: "JP".into(),
                score: 80,
            },
            Player {
                id: 3,
                handle: "golem".into(),
                region: "US".into(),
                score: 60,
            },
        ])?;
        Ok(())
    })?;

    assert_eq!(db.players.by_region("JP").count(), 2);
    assert_eq!(db.players.score_range(50..=80).count(), 2);
    assert_eq!(db.players.by_region_score(("JP", 50..)).count(), 1);
    Ok(())
}
