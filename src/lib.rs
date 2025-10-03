mod db;

use std::collections::HashSet;

pub use db::{Db, Event, IndexedLog};
use futures::TryStreamExt;
use reth_ethereum::{
    EthPrimitives,
    exex::{ExExContext, ExExEvent, ExExHead, ExExNotification},
    node::api::{FullNodeComponents, NodeTypes},
};
use reth_tracing::tracing::{info, warn};

pub async fn indexer<Node, E>(mut ctx: ExExContext<Node>, mut db: Db<E>) -> eyre::Result<()>
where
    Node: FullNodeComponents<Types: NodeTypes<Primitives = EthPrimitives>>,
    E: Event,
{
    if let Some(block) = db.head()? {
        ctx.set_notifications_with_head(ExExHead {
            block: block.into(),
        });
    }

    while let Some(notification) = ctx.notifications.try_next().await? {
        match &notification {
            ExExNotification::ChainCommitted { new } => {
                info!(range = ?new.range(), tip = %new.tip().hash(), "commit");
            }
            ExExNotification::ChainReorged { old, new } => {
                let kept: HashSet<_> = new.inner().0.transaction_hashes().collect();
                let dropped = old
                    .inner()
                    .0
                    .transaction_hashes()
                    .filter(|tx| !kept.contains(tx))
                    .count();

                warn!(
                    fork = ?old.fork_block(),
                    depth = old.len(),
                    old_range = ?old.range(),
                    new_range = ?new.range(),
                    old_tip = %old.tip().hash(),
                    new_tip = %new.tip().hash(),
                    dropped_txs = dropped,
                    "reorg"
                );
            }
            ExExNotification::ChainReverted { old } => {
                warn!(
                    fork = ?old.fork_block(),
                    depth = old.len(),
                    range = ?old.range(),
                    "revert"
                );
            }
        }

        db.apply(&notification)?;

        if let Some(chain) = notification.committed_chain() {
            ctx.events
                .send(ExExEvent::FinishedHeight(chain.tip().num_hash()))?;
        }
    }

    Ok(())
}
