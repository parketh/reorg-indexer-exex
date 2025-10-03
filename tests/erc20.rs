use std::sync::Arc;

use alloy_consensus::{Signed, TxLegacy};
use alloy_primitives::{Address, B256, Log, Signature, U256, address, b256};
use reorg_indexer_exex::{Db, Event, IndexedLog};
use reth_ethereum::{
    Block, BlockBody, Receipt, TransactionSigned,
    exex::ExExNotification,
    primitives::{Header, RecoveredBlock},
    provider::{Chain, ExecutionOutcome},
};
use rusqlite::{Transaction, params};

const TRANSFER: B256 = b256!("ddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef");
const TOKEN: Address = address!("a0b86991c6218b36c1d19d4a2e9eb0ce3606eb48");

struct Transfers;

impl Event for Transfers {
    const MIGRATION: &'static str = "CREATE TABLE IF NOT EXISTS transfers (
        block_number INTEGER NOT NULL,
        block_hash TEXT NOT NULL,
        tx_hash TEXT NOT NULL,
        log_index INTEGER NOT NULL,
        token TEXT NOT NULL,
        sender TEXT NOT NULL,
        recipient TEXT NOT NULL,
        amount TEXT NOT NULL,
        PRIMARY KEY (block_number, log_index)
    );";

    fn index(&self, db: &Transaction<'_>, entry: &IndexedLog<'_>) -> eyre::Result<()> {
        let log = entry.log;
        let [topic, from, to] = log.topics() else {
            return Ok(());
        };
        if *topic != TRANSFER || log.data.data.len() != 32 {
            return Ok(());
        }

        db.execute(
            "INSERT OR REPLACE INTO transfers VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                entry.block_number,
                entry.block_hash.to_string(),
                entry.tx_hash.to_string(),
                entry.log_index,
                log.address.to_string(),
                Address::from_word(*from).to_string(),
                Address::from_word(*to).to_string(),
                U256::from_be_slice(&log.data.data).to_string(),
            ],
        )?;
        Ok(())
    }

    fn revert(&self, db: &Transaction<'_>, from_block: u64) -> eyre::Result<()> {
        db.execute(
            "DELETE FROM transfers WHERE block_number >= ?1",
            [from_block],
        )?;
        Ok(())
    }
}

fn block(number: u64, parent: B256, amount: u64) -> (RecoveredBlock<Block>, Vec<Receipt>) {
    let tx = TransactionSigned::Legacy(Signed::new_unhashed(
        TxLegacy {
            nonce: amount,
            ..Default::default()
        },
        Signature::test_signature(),
    ));
    let header = Header {
        number,
        parent_hash: parent,
        timestamp: amount,
        ..Default::default()
    };
    let body = BlockBody {
        transactions: vec![tx],
        ..Default::default()
    };

    let log = Log::new_unchecked(
        TOKEN,
        vec![
            TRANSFER,
            Address::with_last_byte(1).into_word(),
            Address::with_last_byte(2).into_word(),
        ],
        U256::from(amount).to_be_bytes_vec().into(),
    );
    let receipt = Receipt {
        logs: vec![log],
        ..Default::default()
    };

    (
        RecoveredBlock::new_unhashed(Block { header, body }, vec![Address::ZERO]),
        vec![receipt],
    )
}

fn chain(blocks: Vec<(RecoveredBlock<Block>, Vec<Receipt>)>) -> Arc<Chain> {
    let first = blocks[0].0.num_hash().number;
    let (blocks, receipts): (Vec<_>, Vec<_>) = blocks.into_iter().unzip();
    let outcome = ExecutionOutcome::new(Default::default(), receipts, first, vec![]);
    Arc::new(Chain::new(blocks, outcome, None))
}

fn amounts(db: &Db<Transfers>) -> eyre::Result<Vec<String>> {
    let mut stmt = db
        .conn()
        .prepare("SELECT amount FROM transfers ORDER BY block_number")?;
    let rows = stmt.query_map([], |row| row.get(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

#[test]
fn follows_commits_reorgs_and_reverts() -> eyre::Result<()> {
    let mut db = Db::open(":memory:", Transfers)?;

    let b1 = block(1, B256::ZERO, 100);
    let b2 = block(2, b1.0.hash(), 200);
    let b2_new = block(2, b1.0.hash(), 201);
    let b3_new = block(3, b2_new.0.hash(), 300);
    let (b2_new_hash, b3_new_hash) = (b2_new.0.hash(), b3_new.0.hash());

    db.apply(&ExExNotification::ChainCommitted {
        new: chain(vec![b1, b2.clone()]),
    })?;
    assert_eq!(amounts(&db)?, ["100", "200"]);

    db.apply(&ExExNotification::ChainReorged {
        old: chain(vec![b2]),
        new: chain(vec![b2_new, b3_new.clone()]),
    })?;
    assert_eq!(amounts(&db)?, ["100", "201", "300"]);
    assert_eq!(db.head()?, Some((3, b3_new_hash)));

    db.apply(&ExExNotification::ChainReverted {
        old: chain(vec![b3_new]),
    })?;
    assert_eq!(amounts(&db)?, ["100", "201"]);
    assert_eq!(db.head()?, Some((2, b2_new_hash)));

    Ok(())
}
