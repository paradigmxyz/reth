mod db;

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
use reth_node_ethereum::{EthEvmConfig, EthereumNode};
use reth_primitives::{
    address, constants, Address, Block, BlockWithSenders, Bytes, ChainSpec, ChainSpecBuilder,
    Genesis, Hardfork, Header, Receipt, SealedBlockWithSenders, TransactionSigned, U256,
};
use reth_provider::{BlockExecutor, Chain, ProviderError};
use reth_tracing::tracing::{error, info};
use rusqlite::Connection;

sol!(RollupContract, "rollup_abi.json");
use RollupContract::{RollupContractCalls, RollupContractEvents};

const ROLLUP_CONTRACT_ADDRESS: Address = address!("74ae65DF20cB0e3BF8c022051d0Cdd79cc60890C");
const ROLLUP_SUBMITTER_ADDRESS: Address = address!("B01042Db06b04d3677564222010DF5Bd09C5A947");
const CHAIN_ID: u64 = 17001;
static CHAIN_SPEC: Lazy<Arc<ChainSpec>> = Lazy::new(|| {
    Arc::new(
        ChainSpecBuilder::default()
            .chain(CHAIN_ID.into())
            .genesis(Genesis::clique_genesis(CHAIN_ID, ROLLUP_SUBMITTER_ADDRESS))
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
                        match execute_block(&mut self.db, &header, blockData) {
                            Ok((block, bundle, _)) => {
                                let block = block.seal_slow();
                                self.db.insert_block_with_bundle(&block, bundle)?;
                                info!(
                                    transactions = block.body.len(),
                                    "Block submitted, executed and inserted into database"
                                );
                            }
                            Err(err) => {
                                error!(
                                    %err,
                                    tx_hash = %tx.hash,
                                    chain_id = %header.rollupChainId,
                                    sequence = %header.sequence,
                                    "Failed to execute block"
                                );
                            }
                        }
                    }
                }
                RollupContractEvents::Enter(RollupContract::Enter {
                    token,
                    rollupRecipient,
                    amount,
                }) => {
                    if token != Address::ZERO {
                        error!(tx_hash = %tx.hash, "Only ETH deposits are supported");
                        continue
                    }

                    self.db.upsert_account(rollupRecipient, |account| {
                        let mut account = account.unwrap_or_default();
                        account.balance += amount;
                        Ok(account)
                    })?;

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
                        error!(tx_hash = %tx.hash, "Only ETH withdrawals are supported");
                        continue
                    }

                    self.db.upsert_account(hostRecipient, |account| {
                        let mut account = account.ok_or(eyre::eyre!("account not found"))?;
                        account.balance -= amount;
                        Ok(account)
                    })?;

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
    header: &RollupContract::BlockHeader,
    block_data: Bytes,
) -> eyre::Result<(BlockWithSenders, BundleState, Vec<Receipt>)> {
    if header.rollupChainId != U256::from(CHAIN_ID) {
        eyre::bail!("Invalid rollup chain ID")
    }

    let block_number = u64::try_from(header.sequence)?;
    let parent_block = if !header.sequence.is_zero() {
        db.get_block(header.sequence - U256::from(1))?
    } else {
        None
    };

    // Calculate base fee per gas for EIP-1559 transactions
    let base_fee_per_gas = if CHAIN_SPEC.fork(Hardfork::London).transitions_at_block(block_number) {
        constants::EIP1559_INITIAL_BASE_FEE
    } else {
        parent_block
            .as_ref()
            .ok_or(eyre::eyre!("parent block not found"))?
            .header
            .next_block_base_fee(CHAIN_SPEC.base_fee_params_at_block(block_number))
            .ok_or(eyre::eyre!("failed to calculate base fee"))?
    };

    // Construct header
    let header = Header {
        parent_hash: parent_block.map(|block| block.header.hash()).unwrap_or_default(),
        number: block_number,
        gas_limit: u64::try_from(header.gasLimit)?,
        timestamp: u64::try_from(header.confirmBy)?,
        base_fee_per_gas: Some(base_fee_per_gas),
        ..Default::default()
    };

    // Decode block data and filter only transactions with the correct chain ID
    let body = Vec::<TransactionSigned>::decode(&mut block_data.as_ref())?
        .into_iter()
        .filter(|tx| tx.chain_id() == Some(CHAIN_ID))
        .collect();

    // Construct block and recover senders
    let block = Block { header, body, ..Default::default() }
        .with_recovered_senders()
        .ok_or_eyre("failed to recover senders")?;

    // Execute block
    let state = StateBuilder::new_with_database(
        Box::new(db) as Box<dyn reth::revm::Database<Error = ProviderError> + Send>
    )
    .with_bundle_update()
    .build();
    let mut evm = EVMProcessor::new_with_state(CHAIN_SPEC.clone(), state, EthEvmConfig::default());
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
    use std::time::{SystemTime, UNIX_EPOCH};

    use reth_interfaces::test_utils::generators::{self, sign_tx_with_key_pair};
    use reth_primitives::{
        address, constants::ETH_TO_WEI, public_key_to_address, revm_primitives::AccountInfo,
        Transaction, TransactionKind, TxEip1559, U256,
    };
    use rusqlite::Connection;
    use secp256k1::{KeyPair, Secp256k1};

    use crate::{
        db::Database, execute_block, RollupContract::BlockHeader, CHAIN_ID,
        ROLLUP_SUBMITTER_ADDRESS,
    };

    #[test]
    fn test_execute_block() -> eyre::Result<()> {
        reth_tracing::init_test_tracing();

        let mut database = Database::new(Connection::open_in_memory()?)?;

        // Create key pair
        let secp = Secp256k1::new();
        let key_pair = KeyPair::new(&secp, &mut generators::rng());
        let sender_address = public_key_to_address(key_pair.public_key());

        let recipient_address = address!("Df79e78BF4868b06300074Ebf34002d228A908D9");

        // Deposit some ETH to the sender and insert it into database
        let sender =
            AccountInfo { balance: U256::from(ETH_TO_WEI), nonce: 1, ..Default::default() };
        database.upsert_account(sender_address, |account| {
            if account.is_some() {
                eyre::bail!("account already exists")
            }
            Ok(sender.clone())
        })?;

        // Ensure that recipient does not exist
        let recipient = database.get_account(recipient_address)?;
        assert!(recipient.is_none());

        // Construct and sign transaction
        let tx = Transaction::Eip1559(TxEip1559 {
            chain_id: CHAIN_ID,
            nonce: sender.nonce,
            gas_limit: 21000,
            max_fee_per_gas: 1_500_000_013,
            max_priority_fee_per_gas: 1_500_000_000,
            to: TransactionKind::Call(recipient_address),
            value: U256::from(0.1 * ETH_TO_WEI as f64),
            ..Default::default()
        });
        let signed_tx = sign_tx_with_key_pair(key_pair, tx.clone());

        // Construct block header and data
        let block_header = BlockHeader {
            rollupChainId: U256::from(CHAIN_ID),
            sequence: U256::ZERO,
            confirmBy: U256::from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs()),
            gasLimit: U256::from(30_000_000),
            rewardAddress: ROLLUP_SUBMITTER_ADDRESS,
        };
        let block_data = alloy_rlp::encode(vec![signed_tx.envelope_encoded()]);

        // Execute block and insert into database
        let (block, bundle, _) = execute_block(&mut database, &block_header, block_data.into())?;
        let block = block.seal_slow();
        database.insert_block_with_bundle(&block, bundle)?;

        // Verify new sender balances and nonces
        let sender_new =
            database.get_account(sender_address)?.ok_or(eyre::eyre!("sender not found"))?;
        assert_eq!(
            sender_new.balance,
            // Initial balance
            sender.balance -
            // Transfer value
            tx.value() -
            // Gas fee
            U256::from(tx.gas_limit()) * U256::from(tx.effective_gas_price(block.base_fee_per_gas))
        );
        assert_eq!(sender_new.nonce, 2);

        // Verify new recipient balances and nonces
        let recipient_new =
            database.get_account(recipient_address)?.ok_or(eyre::eyre!("recipient not found"))?;
        assert_eq!(recipient_new.balance, tx.value());
        assert_eq!(recipient_new.nonce, 0);

        Ok(())
    }
}
