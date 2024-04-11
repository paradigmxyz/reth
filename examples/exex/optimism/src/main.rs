use std::{
    pin::Pin,
    task::{ready, Context, Poll},
};

use alloy_sol_types::{sol, SolEventInterface};
use futures::Future;
use reth::builder::FullNodeTypes;
use reth_exex::{ExExContext, ExExEvent};
use reth_node_ethereum::EthereumNode;
use reth_primitives::{Log, SealedBlockWithSenders, TransactionSigned};
use reth_provider::Chain;
use reth_tracing::tracing::info;
use rusqlite::Connection;

sol!(L1StandardBridge, "l1_standard_bridge_abi.json");
use crate::L1StandardBridge::{ETHBridgeFinalized, ETHBridgeInitiated, L1StandardBridgeEvents};

struct OptimismExEx<Node: FullNodeTypes> {
    ctx: ExExContext<Node>,
    connection: Connection,
}

impl<Node: FullNodeTypes> OptimismExEx<Node> {
    fn new(ctx: ExExContext<Node>, connection: Connection) -> eyre::Result<Self> {
        // Create deposits and withdrawals tables
        connection.execute(
            r#"
            CREATE TABLE IF NOT EXISTS deposits (
                id               INTEGER PRIMARY KEY,
                block_number     INTEGER NOT NULL,
                tx_hash          TEXT NOT NULL UNIQUE,
                contract_address TEXT NOT NULL,
                "from"           TEXT NOT NULL,
                "to"             TEXT NOT NULL,
                amount           TEXT NOT NULL
            );
            "#,
            (),
        )?;
        connection.execute(
            r#"
            CREATE TABLE IF NOT EXISTS withdrawals (
                id               INTEGER PRIMARY KEY,
                block_number     INTEGER NOT NULL,
                tx_hash          TEXT NOT NULL UNIQUE,
                contract_address TEXT NOT NULL,
                "from"           TEXT NOT NULL,
                "to"             TEXT NOT NULL,
                amount           TEXT NOT NULL
            );
            "#,
            (),
        )?;

        // Create a bridge contract addresses table and insert known ones with their respective
        // names
        connection.execute(
            r#"
            CREATE TABLE IF NOT EXISTS contracts (
                id              INTEGER PRIMARY KEY,
                address         TEXT NOT NULL UNIQUE,
                name            TEXT NOT NULL
            );
            "#,
            (),
        )?;
        connection.execute(
            r#"
            INSERT OR IGNORE INTO contracts (address, name)
            VALUES
                ('0x3154Cf16ccdb4C6d922629664174b904d80F2C35', 'Base'),
                ('0x3a05E5d33d7Ab3864D53aaEc93c8301C1Fa49115', 'Blast'),
                ('0x697402166Fbf2F22E970df8a6486Ef171dbfc524', 'Blast'),
                ('0x99C9fc46f92E8a1c0deC1b1747d010903E884bE1', 'Optimism'),
                ('0x735aDBbE72226BD52e818E7181953f42E3b0FF21', 'Mode'),
                ('0x3B95bC951EE0f553ba487327278cAc44f29715E5', 'Manta');
            "#,
            (),
        )?;

        info!("Initialized database tables");

        Ok(Self { ctx, connection })
    }
}

impl<Node: FullNodeTypes> Future for OptimismExEx<Node> {
    type Output = eyre::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        // Process all new chain state notifications until there are no more
        while let Some(notification) = ready!(this.ctx.notifications.poll_recv(cx)) {
            if let Some(reverted_chain) = notification.reverted() {
                let events = decode_chain_into_events(&reverted_chain);

                let mut deposits = 0;
                let mut withdrawals = 0;

                for (_, tx, _, event) in events {
                    match event {
                        L1StandardBridgeEvents::ETHBridgeInitiated(ETHBridgeInitiated {
                            ..
                        }) => {
                            deposits += this.connection.execute(
                                "DELETE FROM deposits WHERE tx_hash = ?;",
                                (tx.hash().to_string(),),
                            )?;
                        }
                        L1StandardBridgeEvents::ETHBridgeFinalized(ETHBridgeFinalized {
                            ..
                        }) => {
                            withdrawals += this.connection.execute(
                                "DELETE FROM withdrawals WHERE tx_hash = ?;",
                                (tx.hash().to_string(),),
                            )?;
                        }
                        _ => continue,
                    };
                }

                info!(%deposits, %withdrawals, "Reverted chain events");
            }

            let committed = notification.committed();
            let events = decode_chain_into_events(&committed);

            let mut deposits = 0;
            let mut withdrawals = 0;

            for (block, tx, log, event) in events {
                match event {
                    L1StandardBridgeEvents::ETHBridgeInitiated(ETHBridgeInitiated {
                        amount,
                        from,
                        to,
                        ..
                    }) => {
                        deposits += this.connection.execute(
                                r#"
                                INSERT INTO deposits (block_number, tx_hash, contract_address, "from", "to", amount)
                                VALUES (?, ?, ?, ?, ?, ?)
                                "#,
                                (
                                    block.number,
                                    tx.hash().to_string(),
                                    log.address.to_string(),
                                    from.to_string(),
                                    to.to_string(),
                                    amount.to_string(),
                                ),
                            )?;
                    }
                    L1StandardBridgeEvents::ETHBridgeFinalized(ETHBridgeFinalized {
                        amount,
                        from,
                        to,
                        ..
                    }) => {
                        withdrawals += this.connection.execute(
                                r#"
                                INSERT INTO withdrawals (block_number, tx_hash, contract_address, "from", "to", amount)
                                VALUES (?, ?, ?, ?, ?, ?)
                                "#,
                                (
                                    block.number,
                                    tx.hash().to_string(),
                                    log.address.to_string(),
                                    from.to_string(),
                                    to.to_string(),
                                    amount.to_string(),
                                ),
                            )?;
                    }
                    _ => continue,
                };
            }

            info!(%deposits, %withdrawals, "Committed chain events");

            // Send a finished height event, signaling the node that we don't need any blocks below
            // this height anymore
            this.ctx.events.send(ExExEvent::FinishedHeight(notification.tip().number))?;
        }

        Poll::Pending
    }
}

fn decode_chain_into_events(
    chain: &Chain,
) -> impl Iterator<Item = (&SealedBlockWithSenders, &TransactionSigned, &Log, L1StandardBridgeEvents)>
{
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
        // Get all logs
        .flat_map(|(block, tx, receipt)| receipt.logs.iter().map(move |log| (block, tx, log)))
        // Decode and filter bridge events
        .filter_map(|(block, tx, log)| {
            L1StandardBridgeEvents::decode_raw_log(&log.topics, &log.data, true)
                .ok()
                .map(|event| (block, tx, log, event))
        })
}

fn main() -> eyre::Result<()> {
    reth::cli::Cli::parse_args().run(|builder, _| async move {
        let handle = builder
            .node(EthereumNode::default())
            .install_exex("Optimism", move |ctx| async {
                let connection = Connection::open("optimism.db")?;
                OptimismExEx::new(ctx, connection)
            })
            .launch()
            .await?;

        handle.wait_for_node_exit().await
    })
}
