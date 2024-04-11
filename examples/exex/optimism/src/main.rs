use std::{
    pin::Pin,
    task::{Context, Poll},
};

use alloy_sol_types::{sol, SolEventInterface};
use futures::Future;
use reth::builder::FullNodeTypes;
use reth_exex::{ExExContext, ExExEvent};
use reth_node_ethereum::EthereumNode;
use reth_primitives::{address, Address, Log, TxHash};
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
        let result = connection.execute(
            r#"
            CREATE TABLE IF NOT EXISTS deposits (
                id               INTEGER PRIMARY KEY,
                tx_hash          TEXT NOT NULL,
                contract_address TEXT NOT NULL,
                "from"           TEXT NOT NULL,
                "to"             TEXT NOT NULL,
                amount           TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS withdrawals (
                id               INTEGER PRIMARY KEY,
                tx_hash          TEXT NOT NULL,
                contract_address TEXT NOT NULL,
                "from            TEXT NOT NULL,
                "to"             TEXT NOT NULL,
                amount           TEXT NOT NULL
            );
            "#,
            (),
        )?;

        info!(?result, "Initialized database tables");

        Ok(Self { ctx, connection })
    }
}

impl<Node: FullNodeTypes> Future for OptimismExEx<Node> {
    type Output = eyre::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        // Process all new chain state notifications until there are no more
        while let Poll::Ready(Some(notification)) = this.ctx.notifications.poll_recv(cx) {
            if let Some(reverted_chain) = notification.reverted() {
                let events = decode_chain_into_events(&reverted_chain);

                let mut deposits = 0;
                let mut withdrawals = 0;

                for (tx_hash, _, event) in events {
                    match event {
                        L1StandardBridgeEvents::ETHBridgeInitiated(ETHBridgeInitiated {
                            ..
                        }) => {
                            deposits += this.connection.execute(
                                "DELETE FROM deposits WHERE tx_hash = ?",
                                (tx_hash.to_string(),),
                            )?;
                        }
                        L1StandardBridgeEvents::ETHBridgeFinalized(ETHBridgeFinalized {
                            ..
                        }) => {
                            withdrawals += this.connection.execute(
                                "DELETE FROM withdrawals WHERE tx_hash = ?",
                                (tx_hash.to_string(),),
                            )?;
                        }
                        _ => continue,
                    };
                }

                info!(%deposits, %withdrawals, "Reverted chain events");
            }

            if let Some(committed) = notification.committed() {
                let events = decode_chain_into_events(&committed);

                let mut deposits = 0;
                let mut withdrawals = 0;

                for (tx_hash, log, event) in events {
                    match event {
                        L1StandardBridgeEvents::ETHBridgeInitiated(ETHBridgeInitiated {
                            amount,
                            from,
                            to,
                            ..
                        }) => {
                            deposits += this.connection.execute(
                                r#"INSERT INTO deposits (tx_hash, contract_address, "from", "to", amount) VALUES (?, ?, ?, ?, ?)"#,
                                (
                                    tx_hash.to_string(),
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
                                r#"INSERT INTO withdrawals (tx_hash, contract_address, "from", "to", amount) VALUES (?, ?, ?, ?, ?)"#,
                                (
                                    tx_hash.to_string(),
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
            }

            // Send a finished height event, signaling the node that we don't need any blocks below
            // this height anymore
            this.ctx.events.send(ExExEvent::FinishedHeight(notification.tip().number))?;
        }

        Poll::Pending
    }
}

fn decode_chain_into_events(
    chain: &Chain,
) -> impl Iterator<Item = (TxHash, &Log, L1StandardBridgeEvents)> {
    chain
        // Get all blocks and receipts
        .blocks_and_receipts()
        // Get all receipts
        .flat_map(|(block, receipts)| {
            block.body.iter().map(|transaction| transaction.hash()).zip(receipts.iter().flatten())
        })
        // Get all logs
        .flat_map(|(tx_hash, receipt)| receipt.logs.iter().map(move |log| (tx_hash, log)))
        // Decode and filter bridge events
        .filter_map(|(tx_hash, log)| {
            L1StandardBridgeEvents::decode_raw_log(&log.topics, &log.data, true)
                .ok()
                .map(|event| (tx_hash, log, event))
        })
}

fn contract_address_to_name(address: Address) -> Option<&'static str> {
    const BASE: Address = address!("3154Cf16ccdb4C6d922629664174b904d80F2C35");
    const BLAST_1: Address = address!("3a05E5d33d7Ab3864D53aaEc93c8301C1Fa49115");
    const BLAST_2: Address = address!("697402166Fbf2F22E970df8a6486Ef171dbfc524");
    const OPTIMISM: Address = address!("99C9fc46f92E8a1c0deC1b1747d010903E884bE1");
    const MODE: Address = address!("735aDBbE72226BD52e818E7181953f42E3b0FF21");
    const MANTA: Address = address!("3B95bC951EE0f553ba487327278cAc44f29715E5");

    match address {
        BASE => Some("Base"),
        BLAST_1 | BLAST_2 => Some("Blast"),
        OPTIMISM => Some("Optimism"),
        MODE => Some("Mode"),
        MANTA => Some("Manta"),
        _ => None,
    }
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
