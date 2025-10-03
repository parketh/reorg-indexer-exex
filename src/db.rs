use std::path::Path;

use alloy_primitives::{B256, Log};
use reth_ethereum::{EthPrimitives, exex::ExExNotification};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

pub struct IndexedLog<'a> {
    pub block_number: u64,
    pub block_hash: B256,
    pub tx_hash: B256,
    pub log_index: u64,
    pub log: &'a Log,
}

pub trait Event {
    const MIGRATION: &'static str;

    fn index(&self, db: &Transaction<'_>, log: &IndexedLog<'_>) -> eyre::Result<()>;

    fn revert(&self, db: &Transaction<'_>, from_block: u64) -> eyre::Result<()>;
}

pub struct Db<E> {
    conn: Connection,
    event: E,
}

impl<E: Event> Db<E> {
    pub fn open(path: impl AsRef<Path>, event: E) -> eyre::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS head (
                id INTEGER PRIMARY KEY CHECK (id = 0),
                number INTEGER NOT NULL,
                hash TEXT NOT NULL
            );",
        )?;
        conn.execute_batch(E::MIGRATION)?;
        Ok(Self { conn, event })
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn head(&self) -> eyre::Result<Option<(u64, B256)>> {
        let head = self
            .conn
            .query_row("SELECT number, hash FROM head", [], |row| {
                Ok((row.get::<_, u64>(0)?, row.get::<_, String>(1)?))
            })
            .optional()?;

        match head {
            Some((number, hash)) => Ok(Some((number, hash.parse()?))),
            None => Ok(None),
        }
    }

    pub fn apply(&mut self, notification: &ExExNotification<EthPrimitives>) -> eyre::Result<()> {
        let db = self.conn.transaction()?;

        if let Some(old) = notification.reverted_chain() {
            self.event.revert(&db, *old.range().start())?;
            let fork = old.fork_block();
            Self::set_head(&db, fork.number, fork.hash)?;
        }

        if let Some(new) = notification.committed_chain() {
            for (block, receipts) in new.blocks_and_receipts() {
                let id = block.num_hash();
                let logs =
                    block
                        .body()
                        .transactions
                        .iter()
                        .zip(receipts)
                        .flat_map(|(tx, receipt)| {
                            receipt.logs.iter().map(move |log| (tx.tx_hash(), log))
                        });

                for (log_index, (tx_hash, log)) in logs.enumerate() {
                    self.event.index(
                        &db,
                        &IndexedLog {
                            block_number: id.number,
                            block_hash: id.hash,
                            tx_hash: *tx_hash,
                            log_index: log_index as u64,
                            log,
                        },
                    )?;
                }
            }
            let tip = new.tip().num_hash();
            Self::set_head(&db, tip.number, tip.hash)?;
        }

        db.commit()?;
        Ok(())
    }

    fn set_head(db: &Transaction<'_>, number: u64, hash: B256) -> eyre::Result<()> {
        db.execute(
            "INSERT OR REPLACE INTO head VALUES (0, ?1, ?2)",
            params![number, hash.to_string()],
        )?;
        Ok(())
    }
}
