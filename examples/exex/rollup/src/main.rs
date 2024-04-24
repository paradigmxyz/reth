mod db;
mod evm;

use std::sync::Arc;

use alloy_rlp::Decodable;
use alloy_sol_types::{sol, SolEventInterface, SolInterface};
use db::Database;
use eyre::OptionExt;
use once_cell::sync::Lazy;
use reth::revm::{
    db::{states::bundle_state::BundleRetention, BundleState},
    processor::EVMProcessor,
    StateBuilder,
};
use reth_exex::{ExExContext, ExExEvent};
use reth_node_api::FullNodeComponents;
use reth_node_ethereum::EthereumNode;
use reth_primitives::{
    address, Address, Block, BlockWithSenders, Bytes, ChainSpec, ChainSpecBuilder, Genesis, Header,
    Receipt, SealedBlockWithSenders, TransactionSigned, U256,
};
use reth_provider::{BlockExecutor, Chain, ProviderError};
use reth_tracing::tracing::info;
use rusqlite::Connection;

sol!(RollupContract, "rollup_abi.json");
use RollupContract::{RollupContractCalls, RollupContractEvents};

const ROLLUP_CONTRACT_ADDRESS: Address = address!("74ae65DF20cB0e3BF8c022051d0Cdd79cc60890C");
// TODO(alexey): Use the correct chain ID 17001
const CHAIN_ID: u64 = 17000;
static CHAIN_SPEC: Lazy<Arc<ChainSpec>> = Lazy::new(|| {
    Arc::new(
        ChainSpecBuilder::default()
            .chain(CHAIN_ID.into())
            .genesis(Genesis::clique_genesis(
                CHAIN_ID,
                address!("B01042Db06b04d3677564222010DF5Bd09C5A947"),
            ))
            .london_activated()
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
        while let Some(notification) = self.ctx.notifications.recv().await {
            if let Some(reverted_chan) = notification.reverted_chain() {
                self.revert(&reverted_chan)?;
            }

            if let Some(committed_chain) = notification.committed_chain() {
                self.commit(&committed_chain)?;
                self.ctx.events.send(ExExEvent::FinishedHeight(committed_chain.tip().number))?;
            }
        }

        Ok(())
    }

    fn commit(&mut self, chain: &Chain) -> eyre::Result<()> {
        let events = decode_chain_into_rollup_events(chain);

        for (_, tx, event) in events {
            match event {
                RollupContractEvents::BlockSubmitted(_) => {
                    let call = RollupContractCalls::abi_decode(tx.input(), true)?;

                    if let RollupContractCalls::submitBlock(RollupContract::submitBlockCall {
                        header,
                        blockData,
                        ..
                    }) = call
                    {
                        let (block, bundle, _) = execute_block(&mut self.db, header, blockData)?;
                        self.db.insert_block(&block, bundle)?;

                        info!(
                            transactions = %block.body.len(),
                            "Block submission",
                        );
                    }
                }
                RollupContractEvents::Enter(RollupContract::Enter {
                    token,
                    rollupRecipient,
                    amount,
                }) => {
                    if token != Address::ZERO {
                        eyre::bail!("Only ETH deposits are supported")
                    }

                    self.db.increment_balance(rollupRecipient, amount)?;

                    info!(
                        %amount,
                        recipient = %rollupRecipient,
                        "Deposit",
                    );
                }
                RollupContractEvents::ExitFilled(RollupContract::ExitFilled {
                    token,
                    hostRecipient,
                    amount,
                }) => {
                    if token != Address::ZERO {
                        eyre::bail!("Only ETH withdrawals are supported")
                    }

                    self.db.decrement_balance(hostRecipient, amount)?;

                    info!(
                        %amount,
                        recipient = %hostRecipient,
                        "Withdrawal",
                    );
                }
                _ => (),
            }
        }

        Ok(())
    }

    fn revert(&mut self, _chain: &Chain) -> eyre::Result<()> {
        unimplemented!(
            "reverts need to be stored in database to be able to act on host chain reverts"
        )
    }
}

/// Decode chain of blocks into a flattened list of receipt logs, filter only transactions to the
/// Rollup contract [ROLLUP_CONTRACT_ADDRESS] and extract [RollupContractEvents].
fn decode_chain_into_rollup_events(
    chain: &Chain,
) -> impl Iterator<Item = (&SealedBlockWithSenders, &TransactionSigned, RollupContractEvents)> {
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
        // Filter only transactions to the rollup contract
        .filter(|(_, tx, _)| tx.to() == Some(ROLLUP_CONTRACT_ADDRESS))
        // Get all logs
        .flat_map(|(block, tx, receipt)| receipt.logs.iter().map(move |log| (block, tx, log)))
        // Decode and filter rollup events
        .filter_map(|(block, tx, log)| {
            RollupContractEvents::decode_raw_log(log.topics(), &log.data.data, true)
                .ok()
                .map(|event| (block, tx, event))
        })
}

/// Execute a rollup block and return (block with recovered senders)[BlockWithSenders], (bundle
/// state)[BundleState] and list of (receipts)[Receipt].
fn execute_block(
    db: &mut Database,
    header: RollupContract::BlockHeader,
    block_data: Bytes,
) -> eyre::Result<(BlockWithSenders, BundleState, Vec<Receipt>)> {
    if header.rollupChainId != U256::from(CHAIN_ID) {
        eyre::bail!("Invalid rollup chain ID")
    }

    let body: Vec<TransactionSigned> = Decodable::decode(&mut block_data.as_ref())?;

    let block = Block {
        header: Header {
            number: u64::try_from(header.sequence)?,
            gas_limit: u64::try_from(header.gasLimit)?,
            ..Default::default()
        },
        body,
        ..Default::default()
    }
    .with_recovered_senders()
    .ok_or_eyre("failed to recover senders")?;

    let state = StateBuilder::new_with_database(
        Box::new(db) as Box<dyn reth::revm::Database<Error = ProviderError> + Send>
    )
    .with_bundle_update()
    .build();
    let mut evm = EVMProcessor::new_with_state(CHAIN_SPEC.clone(), state, evm::Config);
    let (receipts, _) = evm.execute_transactions(&block, U256::ZERO)?;
    evm.db_mut().merge_transitions(BundleRetention::PlainState);

    let bundle = evm.db_mut().take_bundle();

    Ok((block, bundle, receipts))
}

fn main() -> eyre::Result<()> {
    reth::cli::Cli::parse_args().run(|builder, _| async move {
        let handle = builder
            .node(EthereumNode::default())
            .install_exex("Rollup", move |ctx| async {
                let connection = Connection::open("rollup.db")?;
                Ok(Rollup::new(ctx, connection)?.start())
            })
            .launch()
            .await?;

        handle.wait_for_node_exit().await
    })
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use reth_primitives::{address, bytes, constants::ETH_TO_WEI, U256};
    use rusqlite::Connection;

    use crate::{db::Database, execute_block, RollupContract::BlockHeader, CHAIN_ID};

    #[test]
    fn test_submit_block() -> eyre::Result<()> {
        reth_tracing::init_test_tracing();

        let mut database = Database::new(Connection::open_in_memory()?)?;

        // https://holesky.etherscan.io/tx/0x8fef47881e7297bb6d6af9eceb30b9b4e6480f32514ede8aacca4531f0e7a51e
        database.increment_balance(
            address!("A5F4567d8B8E8D7b2cb3c53E33B0460A1BE26Cba"),
            U256::from(0.2 * ETH_TO_WEI as f64),
        )?;

        // https://holesky.etherscan.io/tx/0xabf8cccfbcb9beb00ea8020d86660f28e51aed40cc8916a1ae7455749a187974
        let block_header = BlockHeader {
            rollupChainId: U256::from(CHAIN_ID),
            sequence: U256::ZERO,
            confirmBy: U256::from(1713561648),
            gasLimit: U256::from(30000000),
            rewardAddress: address!("B01042Db06b04d3677564222010DF5Bd09C5A947"),
        };
        let (block, bundle, _) = execute_block(
            &mut database,
            block_header,
            bytes!("f878b87602f8738242680184b2d05e0084e2f4fbfa82520894df79e78bf4868b06300074ebf34002d228a908d987038d7ea4c6800080c001a018dc589d98091e8025d2e3e055a69aaf7e47317aae7e22e89da8aa958506ed8ca0714621376acfd07f9939ffba993718f71fb6d67bd9c66307769771d18e4f2337")
        )?;
        database.insert_block(&block, bundle)?;

        println!("block: {:?}", block);

        let sender = database
            .get_account(address!("A5F4567d8B8E8D7b2cb3c53E33B0460A1BE26Cba"))?
            .ok_or(eyre::eyre!("sender not found"))?;
        // assert_eq!(
        //     sender.balance,
        //     U256::from(0.2 * ETH_TO_WEI as f64) - // Initial balance
        //         U256::from_str("0x38d7ea4c68000")? - // Transfer value
        //         // Gas fee
        // );
        assert_eq!(sender.nonce, 2);

        let recipient = database
            .get_account(address!("Df79e78BF4868b06300074Ebf34002d228A908D9"))?
            .ok_or(eyre::eyre!("recipient not found"))?;
        assert_eq!(recipient.balance, U256::from_str("0x38d7ea4c68000")?);

        Ok(())
    }
}
