mod db;

use std::sync::Arc;

use alloy_rlp::Decodable;
use alloy_sol_types::{sol, SolEventInterface, SolInterface};
use db::Database;
use eyre::OptionExt;
use once_cell::sync::Lazy;
use reth::{
    payload::error::PayloadBuilderError,
    revm::{
        db::{states::bundle_state::BundleRetention, BundleState},
        DatabaseCommit, StateBuilder,
    },
};
use reth_exex::{ExExContext, ExExEvent};
use reth_interfaces::executor::BlockValidationError;
use reth_node_api::{ConfigureEvm, ConfigureEvmEnv, FullNodeComponents};
use reth_node_ethereum::{EthEvmConfig, EthereumNode};
use reth_primitives::{
    address, constants,
    revm::env::fill_tx_env,
    revm_primitives::{CfgEnvWithHandlerCfg, EVMError, ExecutionResult, ResultAndState},
    Address, Block, BlockWithSenders, Bytes, ChainSpec, ChainSpecBuilder, Genesis, Hardfork,
    Header, Receipt, SealedBlockWithSenders, TransactionSigned, U256,
};
use reth_provider::{Chain, ProviderError};
use reth_tracing::tracing::{debug, error, info};
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
                            Ok((block, bundle, _, _)) => {
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
                _ => (),
            }
        }

        Ok(())
    }

    fn revert(&mut self, chain: &Chain) -> eyre::Result<()> {
        let events = decode_chain_into_rollup_events(chain);

        for (_, tx, event) in events {
            match event {
                RollupContractEvents::BlockSubmitted(_) => {
                    let call = RollupContractCalls::abi_decode(tx.input(), true)?;

                    if let RollupContractCalls::submitBlock(RollupContract::submitBlockCall {
                        header,
                        ..
                    }) = call
                    {
                        let block_number = u64::try_from(header.sequence)?;
                        self.db.revert_block(block_number)?;
                        info!(
                            chain_id = %header.rollupChainId,
                            sequence = %header.sequence,
                            "Block reverted"
                        );
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
                        let mut account = account.ok_or(eyre::eyre!("account not found"))?;
                        account.balance -= amount;
                        Ok(account)
                    })?;

                    info!(
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
) -> eyre::Result<(BlockWithSenders, BundleState, Vec<Receipt>, Vec<ExecutionResult>)> {
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

    // Decode block data, filter only transactions with the correct chain ID and recover senders
    let transactions = Vec::<TransactionSigned>::decode(&mut block_data.as_ref())?
        .into_iter()
        .filter(|tx| tx.chain_id() == Some(CHAIN_ID))
        .map(|tx| {
            let sender = tx.recover_signer().ok_or(eyre::eyre!("failed to recover signer"))?;
            Ok((tx, sender))
        })
        .collect::<eyre::Result<Vec<(TransactionSigned, Address)>>>()?;

    // Execute block
    let state = StateBuilder::new_with_database(
        Box::new(db) as Box<dyn reth::revm::Database<Error = ProviderError> + Send>
    )
    .with_bundle_update()
    .build();
    let mut evm = EthEvmConfig::default().evm(state);

    // Set state clear flag.
    evm.db_mut().set_state_clear_flag(
        CHAIN_SPEC.fork(Hardfork::SpuriousDragon).active_at_block(header.number),
    );

    let mut cfg = CfgEnvWithHandlerCfg::new_with_spec_id(evm.cfg().clone(), evm.spec_id());
    EthEvmConfig::fill_cfg_and_block_env(
        &mut cfg,
        evm.block_mut(),
        &CHAIN_SPEC,
        &header,
        U256::ZERO,
    );
    *evm.cfg_mut() = cfg.cfg_env;

    let mut receipts = Vec::with_capacity(transactions.len());
    let mut executed_txs = Vec::with_capacity(transactions.len());
    let mut results = Vec::with_capacity(transactions.len());
    if !transactions.is_empty() {
        let mut cumulative_gas_used = 0;
        for (transaction, sender) in transactions {
            // The sum of the transaction’s gas limit, Tg, and the gas utilized in this block prior,
            // must be no greater than the block’s gasLimit.
            let block_available_gas = header.gas_limit - cumulative_gas_used;
            if transaction.gas_limit() > block_available_gas {
                // TODO(alexey): what to do here?
                return Err(BlockValidationError::TransactionGasLimitMoreThanAvailableBlockGas {
                    transaction_gas_limit: transaction.gas_limit(),
                    block_available_gas,
                }
                .into())
            }
            // Execute transaction.
            // Fill revm structure.
            fill_tx_env(evm.tx_mut(), &transaction, sender);

            let ResultAndState { result, state } = match evm.transact() {
                Ok(result) => result,
                Err(err) => {
                    match err {
                        EVMError::Transaction(err) => {
                            // if the transaction is invalid, we can skip it
                            debug!(%err, ?transaction, "Skipping invalid transaction");
                            continue
                        }
                        err => {
                            // this is an error that we should treat as fatal for this attempt
                            return Err(PayloadBuilderError::EvmExecutionError(err).into())
                        }
                    }
                }
            };

            debug!(?transaction, ?result, ?state, "Executed transaction");

            evm.db_mut().commit(state);

            // append gas used
            cumulative_gas_used += result.gas_used();

            // Push transaction changeset and calculate header bloom filter for receipt.
            #[allow(clippy::needless_update)] // side-effect of optimism fields
            receipts.push(Receipt {
                tx_type: transaction.tx_type(),
                success: result.is_success(),
                cumulative_gas_used,
                logs: result.logs().iter().cloned().map(Into::into).collect(),
                ..Default::default()
            });

            // append transaction to the list of executed transactions
            executed_txs.push(transaction);
            results.push(result);
        }

        evm.db_mut().merge_transitions(BundleRetention::Reverts);
    }

    // Construct block and recover senders
    let block = Block { header, body: executed_txs, ..Default::default() }
        .with_recovered_senders()
        .ok_or_eyre("failed to recover senders")?;

    let bundle = evm.db_mut().take_bundle();

    Ok((block, bundle, receipts, results))
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

    use alloy_sol_types::{sol, SolCall};
    use reth::revm::Evm;
    use reth_interfaces::test_utils::generators::{self, sign_tx_with_key_pair};
    use reth_primitives::{
        bytes,
        constants::ETH_TO_WEI,
        public_key_to_address,
        revm_primitives::{AccountInfo, ExecutionResult, Output, TransactTo, TxEnv},
        BlockNumber, Receipt, SealedBlockWithSenders, Transaction, TxEip2930, TxKind, U256,
    };
    use rusqlite::Connection;
    use secp256k1::{Keypair, Secp256k1};

    use crate::{
        db::Database, execute_block, RollupContract::BlockHeader, CHAIN_ID,
        ROLLUP_SUBMITTER_ADDRESS,
    };

    sol!(
        WETH,
        r#"
[
   {
      "constant":true,
      "inputs":[
         {
            "name":"",
            "type":"address"
         }
      ],
      "name":"balanceOf",
      "outputs":[
         {
            "name":"",
            "type":"uint256"
         }
      ],
      "payable":false,
      "stateMutability":"view",
      "type":"function"
   }
]
        "#
    );

    #[test]
    fn test_execute_block() -> eyre::Result<()> {
        reth_tracing::init_test_tracing();

        let mut database = Database::new(Connection::open_in_memory()?)?;

        // Create key pair
        let secp = Secp256k1::new();
        let key_pair = Keypair::new(&secp, &mut generators::rng());
        let sender_address = public_key_to_address(key_pair.public_key());

        // Deposit some ETH to the sender and insert it into database
        database.upsert_account(sender_address, |_| {
            Ok(AccountInfo { balance: U256::from(ETH_TO_WEI), nonce: 1, ..Default::default() })
        })?;

        // WETH deployment transaction
        let (_, _, results) = execute_transaction(
            &mut database,
            key_pair,
            0,
            Transaction::Eip2930(TxEip2930 {
                chain_id: CHAIN_ID,
                nonce: 1,
                gas_limit: 1_500_000,
                gas_price: 1_500_000_000,
                to: TxKind::Create,
                // WETH9 bytecode
                input: bytes!("60606040526040805190810160405280600d81526020017f57726170706564204574686572000000000000000000000000000000000000008152506000908051906020019061004f9291906100c8565b506040805190810160405280600481526020017f57455448000000000000000000000000000000000000000000000000000000008152506001908051906020019061009b9291906100c8565b506012600260006101000a81548160ff021916908360ff16021790555034156100c357600080fd5b61016d565b828054600181600116156101000203166002900490600052602060002090601f016020900481019282601f1061010957805160ff1916838001178555610137565b82800160010185558215610137579182015b8281111561013657825182559160200191906001019061011b565b5b5090506101449190610148565b5090565b61016a91905b8082111561016657600081600090555060010161014e565b5090565b90565b610c348061017c6000396000f3006060604052600436106100af576000357c0100000000000000000000000000000000000000000000000000000000900463ffffffff16806306fdde03146100b9578063095ea7b31461014757806318160ddd146101a157806323b872dd146101ca5780632e1a7d4d14610243578063313ce5671461026657806370a082311461029557806395d89b41146102e2578063a9059cbb14610370578063d0e30db0146103ca578063dd62ed3e146103d4575b6100b7610440565b005b34156100c457600080fd5b6100cc6104dd565b6040518080602001828103825283818151815260200191508051906020019080838360005b8381101561010c5780820151818401526020810190506100f1565b50505050905090810190601f1680156101395780820380516001836020036101000a031916815260200191505b509250505060405180910390f35b341561015257600080fd5b610187600480803573ffffffffffffffffffffffffffffffffffffffff1690602001909190803590602001909190505061057b565b604051808215151515815260200191505060405180910390f35b34156101ac57600080fd5b6101b461066d565b6040518082815260200191505060405180910390f35b34156101d557600080fd5b610229600480803573ffffffffffffffffffffffffffffffffffffffff1690602001909190803573ffffffffffffffffffffffffffffffffffffffff1690602001909190803590602001909190505061068c565b604051808215151515815260200191505060405180910390f35b341561024e57600080fd5b61026460048080359060200190919050506109d9565b005b341561027157600080fd5b610279610b05565b604051808260ff1660ff16815260200191505060405180910390f35b34156102a057600080fd5b6102cc600480803573ffffffffffffffffffffffffffffffffffffffff16906020019091905050610b18565b6040518082815260200191505060405180910390f35b34156102ed57600080fd5b6102f5610b30565b6040518080602001828103825283818151815260200191508051906020019080838360005b8381101561033557808201518184015260208101905061031a565b50505050905090810190601f1680156103625780820380516001836020036101000a031916815260200191505b509250505060405180910390f35b341561037b57600080fd5b6103b0600480803573ffffffffffffffffffffffffffffffffffffffff16906020019091908035906020019091905050610bce565b604051808215151515815260200191505060405180910390f35b6103d2610440565b005b34156103df57600080fd5b61042a600480803573ffffffffffffffffffffffffffffffffffffffff1690602001909190803573ffffffffffffffffffffffffffffffffffffffff16906020019091905050610be3565b6040518082815260200191505060405180910390f35b34600360003373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff168152602001908152602001600020600082825401925050819055503373ffffffffffffffffffffffffffffffffffffffff167fe1fffcc4923d04b559f4d29a8bfc6cda04eb5b0d3c460751c2402c5c5cc9109c346040518082815260200191505060405180910390a2565b60008054600181600116156101000203166002900480601f0160208091040260200160405190810160405280929190818152602001828054600181600116156101000203166002900480156105735780601f1061054857610100808354040283529160200191610573565b820191906000526020600020905b81548152906001019060200180831161055657829003601f168201915b505050505081565b600081600460003373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002060008573ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff168152602001908152602001600020819055508273ffffffffffffffffffffffffffffffffffffffff163373ffffffffffffffffffffffffffffffffffffffff167f8c5be1e5ebec7d5bd14f71427d1e84f3dd0314c0f7b2291e5b200ac8c7c3b925846040518082815260200191505060405180910390a36001905092915050565b60003073ffffffffffffffffffffffffffffffffffffffff1631905090565b600081600360008673ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002054101515156106dc57600080fd5b3373ffffffffffffffffffffffffffffffffffffffff168473ffffffffffffffffffffffffffffffffffffffff16141580156107b457507fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff600460008673ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002060003373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020019081526020016000205414155b156108cf5781600460008673ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002060003373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff168152602001908152602001600020541015151561084457600080fd5b81600460008673ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002060003373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff168152602001908152602001600020600082825403925050819055505b81600360008673ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020019081526020016000206000828254039250508190555081600360008573ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff168152602001908152602001600020600082825401925050819055508273ffffffffffffffffffffffffffffffffffffffff168473ffffffffffffffffffffffffffffffffffffffff167fddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef846040518082815260200191505060405180910390a3600190509392505050565b80600360003373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020019081526020016000205410151515610a2757600080fd5b80600360003373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff168152602001908152602001600020600082825403925050819055503373ffffffffffffffffffffffffffffffffffffffff166108fc829081150290604051600060405180830381858888f193505050501515610ab457600080fd5b3373ffffffffffffffffffffffffffffffffffffffff167f7fcf532c15f0a6db0bd6d0e038bea71d30d808c7d98cb3bf7268a95bf5081b65826040518082815260200191505060405180910390a250565b600260009054906101000a900460ff1681565b60036020528060005260406000206000915090505481565b60018054600181600116156101000203166002900480601f016020809104026020016040519081016040528092919081815260200182805460018160011615610100020316600290048015610bc65780601f10610b9b57610100808354040283529160200191610bc6565b820191906000526020600020905b815481529060010190602001808311610ba957829003601f168201915b505050505081565b6000610bdb33848461068c565b905092915050565b60046020528160005260406000206020528060005260406000206000915091505054815600a165627a7a72305820deb4c2ccab3c2fdca32ab3f46728389c2fe2c165d5fafa07661e4e004f6c344a0029"),
                ..Default::default()
            })
        )?;

        let weth_address = match results.first() {
            Some(ExecutionResult::Success { output: Output::Create(_, Some(address)), .. }) => {
                *address
            }
            _ => eyre::bail!("WETH contract address not found"),
        };

        // WETH deposit transaction
        execute_transaction(
            &mut database,
            key_pair,
            1,
            Transaction::Eip2930(TxEip2930 {
                chain_id: CHAIN_ID,
                nonce: 2,
                gas_limit: 50000,
                gas_price: 1_500_000_000,
                to: TxKind::Call(weth_address),
                value: U256::from(0.5 * ETH_TO_WEI as f64),
                input: bytes!("d0e30db0"),
                ..Default::default()
            }),
        )?;

        // Verify WETH balance
        let mut evm = Evm::builder()
            .with_db(&mut database)
            .with_tx_env(TxEnv {
                caller: sender_address,
                gas_limit: 50_000_000,
                transact_to: TransactTo::Call(weth_address),
                data: WETH::balanceOfCall::new((sender_address,)).abi_encode().into(),
                ..Default::default()
            })
            .build();
        let result = evm.transact()?.result;
        assert_eq!(
            result.output(),
            Some(&U256::from(0.5 * ETH_TO_WEI as f64).to_be_bytes_vec().into())
        );
        drop(evm);

        // Verify nonce
        let account = database.get_account(sender_address)?.unwrap();
        assert_eq!(account.nonce, 3);

        // Revert block with WETH deposit transaction
        database.revert_block(1)?;

        // Verify WETH balance after revert
        let mut evm = Evm::builder()
            .with_db(&mut database)
            .with_tx_env(TxEnv {
                caller: sender_address,
                gas_limit: 50_000_000,
                transact_to: TransactTo::Call(weth_address),
                data: WETH::balanceOfCall::new((sender_address,)).abi_encode().into(),
                ..Default::default()
            })
            .build();
        let result = evm.transact()?.result;
        assert_eq!(result.output(), Some(&U256::ZERO.to_be_bytes_vec().into()));
        drop(evm);

        // Verify nonce after revert
        let account = database.get_account(sender_address)?.unwrap();
        assert_eq!(account.nonce, 2);

        Ok(())
    }

    fn execute_transaction(
        database: &mut Database,
        key_pair: Keypair,
        sequence: BlockNumber,
        tx: Transaction,
    ) -> eyre::Result<(SealedBlockWithSenders, Vec<Receipt>, Vec<ExecutionResult>)> {
        let signed_tx = sign_tx_with_key_pair(key_pair, tx);

        // Construct block header and data
        let block_header = BlockHeader {
            rollupChainId: U256::from(CHAIN_ID),
            sequence: U256::from(sequence),
            confirmBy: U256::from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs()),
            gasLimit: U256::from(30_000_000),
            rewardAddress: ROLLUP_SUBMITTER_ADDRESS,
        };
        let block_data = alloy_rlp::encode(vec![signed_tx.envelope_encoded()]);

        // Execute block and insert into database
        let (block, bundle, receipts, results) =
            execute_block(database, &block_header, block_data.into())?;
        let block = block.seal_slow();
        database.insert_block_with_bundle(&block, bundle)?;

        Ok((block, receipts, results))
    }
}
