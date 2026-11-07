#![allow(missing_docs)]

use monstermemory::prelude::*;

#[derive(Clone, Debug, Table)]
struct Account {
    #[primary_key]
    id: u64,
    balance: i64,
}

#[derive(Database)]
#[database(validate = "validate_balances")]
struct Bank {
    accounts: Table<Account>,
}

fn validate_balances(snapshot: &BankSnapshot) -> Result<()> {
    if snapshot.accounts.iter().any(|account| account.balance < 0) {
        Err(Error::Configuration(
            "negative balances are forbidden".to_owned(),
        ))
    } else {
        Ok(())
    }
}

#[test]
fn candidate_root_validation_happens_before_publication() -> Result<()> {
    let db = Bank::memory();
    db.accounts.insert(Account { id: 1, balance: 10 })?;
    let sequence = db.snapshot().sequence();
    assert!(
        db.accounts
            .update(&1, |account| account.balance = -1)
            .is_err()
    );
    assert_eq!(db.accounts.get(&1).unwrap().balance, 10);
    assert_eq!(db.snapshot().sequence(), sequence);
    Ok(())
}
