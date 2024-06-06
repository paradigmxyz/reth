use alloy_sol_types::{sol, SolEventInterface};
use futures::Future;
use reth_exex::{ExExContext, ExExEvent};
use reth_node_api::FullNodeComponents;
use reth_node_ethereum::EthereumNode;
use reth_primitives::{address, Address, Log, SealedBlockWithSenders, TransactionSigned};
use reth_provider::Chain;
use reth_tracing::tracing::info;
use rusqlite::Connection;

sol!(L1StandardBridge, "l1_standard_bridge_abi.json");
use crate::L1StandardBridge::{ETHBridgeFinalized, ETHBridgeInitiated, L1StandardBridgeEvents};

const OP_BRIDGES: [Address; 6] = [
    address!("3154Cf16ccdb4C6d922629664174b904d80F2C35"),
    address!("3a05E5d33d7Ab3864D53aaEc93c8301C1Fa49115"),
    address!("697402166Fbf2F22E970df8a6486Ef171dbfc524"),
    address!("99C9fc46f92E8a1c0deC1b1747d010903E884bE1"),
    address!("735aDBbE72226BD52e818E7181953f42E3b0FF21"),
    address!("3B95bC951EE0f553ba487327278cAc44f29715E5"),
];

/// Initializes the ExEx.
///
/// Opens up a SQLite database and creates the tables (if they don't exist).
async fn init<Node: FullNodeComponents>(
    ctx: ExExContext<Node>,
    mut connection: Connection,
) -> eyre::Result<impl Future<Output = eyre::Result<()>>> {
    create_tables(&mut connection)?;

    Ok(op_bridge_exex(ctx, connection))
}

/// Create SQLite tables if they do not exist.
fn create_tables(connection: &mut Connection) -> rusqlite::Result<()> {
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

    Ok(())
}

/// An example of ExEx that listens to ETH bridging events from OP Stack chains
/// and stores deposits and withdrawals in a SQLite database.
async fn op_bridge_exex<Node: FullNodeComponents>(
    mut ctx: ExExContext<Node>,
    connection: Connection,
) -> eyre::Result<()> {
    // Process all new chain state notifications
    while let Some(notification) = ctx.notifications.recv().await {
        // Revert all deposits and withdrawals
        if let Some(reverted_chain) = notification.reverted_chain() {
            let events = decode_chain_into_events(&reverted_chain);

            let mut deposits = 0;
            let mut withdrawals = 0;

            for (_, tx, _, event) in events {
                match event {
                    // L1 -> L2 deposit
                    L1StandardBridgeEvents::ETHBridgeInitiated(ETHBridgeInitiated { .. }) => {
                        let deleted = connection.execute(
                            "DELETE FROM deposits WHERE tx_hash = ?;",
                            (tx.hash().to_string(),),
                        )?;
                        deposits += deleted;
                    }
                    // L2 -> L1 withdrawal
                    L1StandardBridgeEvents::ETHBridgeFinalized(ETHBridgeFinalized { .. }) => {
                        let deleted = connection.execute(
                            "DELETE FROM withdrawals WHERE tx_hash = ?;",
                            (tx.hash().to_string(),),
                        )?;
                        withdrawals += deleted;
                    }
                    _ => continue,
                }
            }

            info!(block_range = ?reverted_chain.range(), %deposits, %withdrawals, "Reverted chain events");
        }

        // Insert all new deposits and withdrawals
        if let Some(committed_chain) = notification.committed_chain() {
            let events = decode_chain_into_events(&committed_chain);

            let mut deposits = 0;
            let mut withdrawals = 0;

            for (block, tx, log, event) in events {
                match event {
                    // L1 -> L2 deposit
                    L1StandardBridgeEvents::ETHBridgeInitiated(ETHBridgeInitiated {
                        amount,
                        from,
                        to,
                        ..
                    }) => {
                        let inserted = connection.execute(
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
                        deposits += inserted;
                    }
                    // L2 -> L1 withdrawal
                    L1StandardBridgeEvents::ETHBridgeFinalized(ETHBridgeFinalized {
                        amount,
                        from,
                        to,
                        ..
                    }) => {
                        let inserted = connection.execute(
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
                        withdrawals += inserted;
                    }
                    _ => continue,
                };
            }

            info!(block_range = ?committed_chain.range(), %deposits, %withdrawals, "Committed chain events");

            // Send a finished height event, signaling the node that we don't need any blocks below
            // this height anymore
            ctx.events.send(ExExEvent::FinishedHeight(committed_chain.tip().number))?;
        }
    }

    Ok(())
}

/// Decode chain of blocks into a flattened list of receipt logs, and filter only
/// [L1StandardBridgeEvents].
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
        // Get all logs from expected bridge contracts
        .flat_map(|(block, tx, receipt)| {
            receipt
                .logs
                .iter()
                .filter(|log| OP_BRIDGES.contains(&log.address))
                .map(move |log| (block, tx, log))
        })
        // Decode and filter bridge events
        .filter_map(|(block, tx, log)| {
            L1StandardBridgeEvents::decode_raw_log(log.topics(), &log.data.data, true)
                .ok()
                .map(|event| (block, tx, log, event))
        })
}

fn main() -> eyre::Result<()> {
    reth::cli::Cli::parse_args().run(|builder, _| async move {
        let handle = builder
            .node(EthereumNode::default())
            .install_exex("OPBridge", |ctx| async move {
                let connection = Connection::open("op_bridge.db")?;
                init(ctx, connection).await
            })
            .launch()
            .await?;

        handle.wait_for_node_exit().await
    })
}

#[cfg(test)]
mod tests {
    use std::pin::pin;

    use alloy_sol_types::SolEvent;
    use reth::revm::db::BundleState;
    use reth_exex_test_utils::{test_exex_context, PollOnce};
    use reth_primitives::{
        Address, Block, Header, Log, Receipt, Receipts, Transaction, TxKind, TxLegacy, TxType, U256,
    };
    use reth_provider::{BundleStateWithReceipts, Chain};
    use reth_testing_utils::generators::sign_tx_with_random_key_pair;
    use rusqlite::Connection;

    use crate::{
        L1StandardBridge::{self},
        OP_BRIDGES,
    };

    #[tokio::test]
    async fn test_exex() -> eyre::Result<()> {
        let (ctx, handle) = test_exex_context().await?;

        let db_file = tempfile::NamedTempFile::new()?;

        let mut exex = pin!(super::init(ctx, Connection::open(&db_file)?).await?);

        let from_address = Address::random();
        let to_address = Address::random();

        let deposit_tx =
            Transaction::Legacy(TxLegacy { to: TxKind::Call(OP_BRIDGES[0]), ..Default::default() });
        let deposit_event = L1StandardBridge::ETHBridgeInitiated {
            from: from_address,
            to: to_address,
            amount: U256::from(100),
            extraData: Default::default(),
        };
        let deposit_log = Log::new(
            OP_BRIDGES[0],
            deposit_event.encode_topics().into_iter().map(|t| t.0).collect(),
            deposit_event.encode_data().into(),
        )
        .ok_or_else(|| eyre::eyre!("failed to encode deposit event"))?;
        let deposit_tx_receipt = Receipt {
            tx_type: TxType::Legacy,
            success: true,
            cumulative_gas_used: 0,
            logs: vec![deposit_log],
            ..Default::default()
        };

        let block = Block {
            header: Header::default(),
            body: vec![sign_tx_with_random_key_pair(&mut rand::thread_rng(), deposit_tx)],
            ..Default::default()
        }
        .seal_slow()
        .seal_with_senders()
        .ok_or_else(|| eyre::eyre!("failed to recover senders"))?;

        handle
            .send_notification_chain_committed(Chain::new(
                vec![block.clone()],
                BundleStateWithReceipts::new(
                    BundleState::default(),
                    Receipts::from_block_receipt(vec![deposit_tx_receipt]),
                    block.number,
                ),
                None,
            ))
            .await?;

        exex.poll_once().await;

        let connection = Connection::open(&db_file)?;

        let rows: Vec<(u64, String, String, String, String, String)> = connection
            .prepare(r#"SELECT block_number, contract_address, "from", "to", amount, tx_hash FROM deposits"#)?
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0],
            (
                block.number,
                OP_BRIDGES[0].to_string(),
                from_address.to_string(),
                to_address.to_string(),
                U256::from(100).to_string(),
                block.body[0].hash().to_string()
            )
        );

        Ok(())
    }
}
