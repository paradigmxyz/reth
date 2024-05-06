//! Example of a simple rollup that derives its state from the L1 chain by executing transactions,
//! processing deposits and storing all related data in an SQLite database.
//!
//! The rollup contract accepts blocks of transactions and deposits of ETH and is deployed on
//! Holesky at [ROLLUP_CONTRACT_ADDRESS], see <https://github.com/init4tech/zenith/blob/e0481e930947513166881a83e276b316c2f38502/src/Zenith.sol>.

use alloy_sol_types::{sol, SolEventInterface, SolInterface};
use db::Database;
use execution::execute_block;
use once_cell::sync::Lazy;
use reth_exex::{ExExContext, ExExEvent};
use reth_node_api::FullNodeComponents;
use reth_node_ethereum::EthereumNode;
use reth_primitives::{
    address, Address, ChainSpec, ChainSpecBuilder, Genesis, SealedBlockWithSenders,
    TransactionSigned, U256,
};
use reth_provider::Chain;
use reth_tracing::tracing::{error, info};
use rusqlite::Connection;
use std::sync::Arc;

mod db;
mod execution;

sol!(RollupContract, "rollup_abi.json");
use RollupContract::{RollupContractCalls, RollupContractEvents};

const DATABASE_PATH: &str = "rollup.db";
const ROLLUP_CONTRACT_ADDRESS: Address = address!("97C0E40c6B5bb5d4fa3e2AA1C6b8bC7EA5ECAe31");
const ROLLUP_SUBMITTER_ADDRESS: Address = address!("5b0517Dc94c413a5871536872605522E54C85a03");
const CHAIN_ID: u64 = 17001;
static CHAIN_SPEC: Lazy<Arc<ChainSpec>> = Lazy::new(|| {
    Arc::new(
        ChainSpecBuilder::default()
            .chain(CHAIN_ID.into())
            .genesis(Genesis::clique_genesis(CHAIN_ID, ROLLUP_SUBMITTER_ADDRESS))
            .shanghai_activated()
            .build(),
    )
});

struct Rollup<Node: FullNodeComponents> {
    ctx: ExExContext<Node>,
    db: Database,
}

impl<Node: FullNodeComponents> Rollup<Node> {
    fn new(ctx: ExExContext<Node>, connection: Connection) -> eyre::Result<Self> {
        let db = Database::new(connection)?;
        Ok(Self { ctx, db })
    }

    async fn start(mut self) -> eyre::Result<()> {
        // Process all new chain state notifications
        while let Some(notification) = self.ctx.notifications.recv().await {
            if let Some(reverted_chain) = notification.reverted_chain() {
                self.revert(&reverted_chain)?;
            }

            if let Some(committed_chain) = notification.committed_chain() {
                self.commit(&committed_chain).await?;
                self.ctx.events.send(ExExEvent::FinishedHeight(committed_chain.tip().number))?;
            }
        }

        Ok(())
    }

    /// Process a new chain commit.
    ///
    /// This function decodes all transactions to the rollup contract into events, executes the
    /// corresponding actions and inserts the results into the database.
    async fn commit(&mut self, chain: &Chain) -> eyre::Result<()> {
        let events = decode_chain_into_rollup_events(chain);

        for (_, tx, event) in events {
            match event {
                // A new block is submitted to the rollup contract.
                // The block is executed on top of existing rollup state and committed into the
                // database.
                RollupContractEvents::BlockSubmitted(RollupContract::BlockSubmitted {
                    blockDataHash,
                    ..
                }) => {
                    let call = RollupContractCalls::abi_decode(tx.input(), true)?;

                    if let RollupContractCalls::submitBlock(RollupContract::submitBlockCall {
                        header,
                        blockData,
                        ..
                    }) = call
                    {
                        match execute_block(
                            &mut self.db,
                            self.ctx.pool(),
                            tx,
                            &header,
                            blockData,
                            blockDataHash,
                        )
                        .await
                        {
                            Ok((block, bundle, _, _)) => {
                                let block = block.seal_slow();
                                self.db.insert_block_with_bundle(&block, bundle)?;
                                info!(
                                    tx_hash = %tx.recalculate_hash(),
                                    chain_id = %header.rollupChainId,
                                    sequence = %header.sequence,
                                    transactions = block.body.len(),
                                    "Block submitted, executed and inserted into database"
                                );
                            }
                            Err(err) => {
                                error!(
                                    %err,
                                    tx_hash = %tx.recalculate_hash(),
                                    chain_id = %header.rollupChainId,
                                    sequence = %header.sequence,
                                    "Failed to execute block"
                                );
                            }
                        }
                    }
                }
                // A deposit of ETH to the rollup contract. The deposit is added to the recipient's
                // balance and committed into the database.
                RollupContractEvents::Enter(RollupContract::Enter {
                    rollupChainId,
                    token,
                    rollupRecipient,
                    amount,
                }) => {
                    if rollupChainId != U256::from(CHAIN_ID) {
                        error!(tx_hash = %tx.recalculate_hash(), "Invalid rollup chain ID");
                        continue
                    }
                    if token != Address::ZERO {
                        error!(tx_hash = %tx.recalculate_hash(), "Only ETH deposits are supported");
                        continue
                    }

                    self.db.upsert_account(rollupRecipient, |account| {
                        let mut account = account.unwrap_or_default();
                        account.balance += amount;
                        Ok(account)
                    })?;

                    info!(
                        tx_hash = %tx.recalculate_hash(),
                        %amount,
                        recipient = %rollupRecipient,
                        "Deposit",
                    );
                }
                _ => (),
            }
        }

        Ok(())
    }

    /// Process a chain revert.
    ///
    /// This function decodes all transactions to the rollup contract into events, reverts the
    /// corresponding actions and updates the database.
    fn revert(&mut self, chain: &Chain) -> eyre::Result<()> {
        let mut events = decode_chain_into_rollup_events(chain);
        // Reverse the order of events to start reverting from the tip
        events.reverse();

        for (_, tx, event) in events {
            match event {
                // The block is reverted from the database.
                RollupContractEvents::BlockSubmitted(_) => {
                    let call = RollupContractCalls::abi_decode(tx.input(), true)?;

                    if let RollupContractCalls::submitBlock(RollupContract::submitBlockCall {
                        header,
                        ..
                    }) = call
                    {
                        self.db.revert_tip_block(header.sequence)?;
                        info!(
                            tx_hash = %tx.recalculate_hash(),
                            chain_id = %header.rollupChainId,
                            sequence = %header.sequence,
                            "Block reverted"
                        );
                    }
                }
                // The deposit is subtracted from the recipient's balance.
                RollupContractEvents::Enter(RollupContract::Enter {
                    rollupChainId,
                    token,
                    rollupRecipient,
                    amount,
                }) => {
                    if rollupChainId != U256::from(CHAIN_ID) {
                        error!(tx_hash = %tx.recalculate_hash(), "Invalid rollup chain ID");
                        continue
                    }
                    if token != Address::ZERO {
                        error!(tx_hash = %tx.recalculate_hash(), "Only ETH deposits are supported");
                        continue
                    }

                    self.db.upsert_account(rollupRecipient, |account| {
                        let mut account = account.ok_or(eyre::eyre!("account not found"))?;
                        account.balance -= amount;
                        Ok(account)
                    })?;

                    info!(
                        tx_hash = %tx.recalculate_hash(),
                        %amount,
                        recipient = %rollupRecipient,
                        "Deposit reverted",
                    );
                }
                _ => (),
            }
        }

        Ok(())
    }
}

/// Decode chain of blocks into a flattened list of receipt logs, filter only transactions to the
/// Rollup contract [ROLLUP_CONTRACT_ADDRESS] and extract [RollupContractEvents].
fn decode_chain_into_rollup_events(
    chain: &Chain,
) -> Vec<(&SealedBlockWithSenders, &TransactionSigned, RollupContractEvents)> {
    chain
        // Get all blocks and receipts
        .blocks_and_receipts()
        // Get all receipts
        .flat_map(|(block, receipts)| {
            block
                .body
                .iter()
                .zip(receipts.iter().flatten())
                .map(move |(tx, receipt)| (block, tx, receipt))
        })
        // Get all logs from rollup contract
        .flat_map(|(block, tx, receipt)| {
            receipt
                .logs
                .iter()
                .filter(|log| log.address == ROLLUP_CONTRACT_ADDRESS)
                .map(move |log| (block, tx, log))
        })
        // Decode and filter rollup events
        .filter_map(|(block, tx, log)| {
            RollupContractEvents::decode_raw_log(log.topics(), &log.data.data, true)
                .ok()
                .map(|event| (block, tx, event))
        })
        .collect()
}

fn main() -> eyre::Result<()> {
    reth::cli::Cli::parse_args().run(|builder, _| async move {
        let handle = builder
            .node(EthereumNode::default())
            .install_exex("Rollup", move |ctx| async {
                let connection = Connection::open(DATABASE_PATH)?;

                Ok(Rollup::new(ctx, connection)?.start())
            })
            .launch()
            .await?;

        handle.wait_for_node_exit().await
    })
}
