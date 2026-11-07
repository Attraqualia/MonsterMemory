#![allow(missing_docs)]

use monstermemory::prelude::*;

#[derive(Clone, Debug, Table)]
struct Account {
    #[primary_key]
    id: u64,
    balance: i64,
}

#[derive(Clone, Debug, Table)]
struct Audit {
    #[primary_key]
    id: u64,
    account_id: u64,
}

#[derive(Database)]
struct Ledger {
    accounts: Table<Account>,
    audits: Table<Audit>,
}

fn main() -> Result<()> {
    let db = Ledger::memory();
    db.accounts.insert(Account {
        id: 1,
        balance: 100,
    })?;
    db.write(|tx| {
        tx.accounts.update(&1, |account| account.balance -= 25)?;
        tx.audits.insert(Audit {
            id: 1,
            account_id: 1,
        })?;
        Ok(())
    })?;
    assert_eq!(db.accounts.get(&1).unwrap().balance, 75);
    assert_eq!(db.audits.get(&1).unwrap().account_id, 1);
    Ok(())
}
