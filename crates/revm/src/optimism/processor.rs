use crate::processor::{compare_receipts_root_and_logs_bloom, EVMProcessor};
use reth_interfaces::executor::{
    BlockExecutionError, BlockValidationError, OptimismBlockExecutionError,
};
use reth_node_api::ConfigureEvmEnv;
use reth_primitives::{
    proofs::calculate_receipt_root_optimism, revm_primitives::ResultAndState, BlockWithSenders,
    Bloom, ChainSpec, Hardfork, Receipt, ReceiptWithBloom, TxType, B256, U256,
};
use reth_provider::{BlockExecutor, BlockExecutorStats, BundleStateWithReceipts};
use revm::DatabaseCommit;
use std::time::Instant;
use tracing::{debug, trace};

/// Verify the calculated receipts root against the expected receipts root.
pub fn verify_receipt_optimism<'a>(
    expected_receipts_root: B256,
    expected_logs_bloom: Bloom,
    receipts: impl Iterator<Item = &'a Receipt> + Clone,
    chain_spec: &ChainSpec,
    timestamp: u64,
) -> Result<(), BlockExecutionError> {
    // Calculate receipts root.
    let receipts_with_bloom = receipts.map(|r| r.clone().into()).collect::<Vec<ReceiptWithBloom>>();
    let receipts_root =
        calculate_receipt_root_optimism(&receipts_with_bloom, chain_spec, timestamp);

    // Create header log bloom.
    let logs_bloom = receipts_with_bloom.iter().fold(Bloom::ZERO, |bloom, r| bloom | r.bloom);

    compare_receipts_root_and_logs_bloom(
        receipts_root,
        logs_bloom,
        expected_receipts_root,
        expected_logs_bloom,
    )?;

    Ok(())
}

impl<'a, EvmConfig> BlockExecutor for EVMProcessor<'a, EvmConfig>
where
    EvmConfig: ConfigureEvmEnv,
{
    fn execute(
        &mut self,
        block: &BlockWithSenders,
        total_difficulty: U256,
    ) -> Result<(), BlockExecutionError> {
        let receipts = self.execute_inner(block, total_difficulty)?;
        self.save_receipts(receipts)
    }

    fn execute_and_verify_receipt(
        &mut self,
        block: &BlockWithSenders,
        total_difficulty: U256,
    ) -> Result<(), BlockExecutionError> {
        // execute block
        let receipts = self.execute_inner(block, total_difficulty)?;

        // TODO Before Byzantium, receipts contained state root that would mean that expensive
        // operation as hashing that is needed for state root got calculated in every
        // transaction This was replaced with is_success flag.
        // See more about EIP here: https://eips.ethereum.org/EIPS/eip-658
        if self.chain_spec.fork(Hardfork::Byzantium).active_at_block(block.header.number) {
            let time = Instant::now();
            if let Err(error) = verify_receipt_optimism(
                block.header.receipts_root,
                block.header.logs_bloom,
                receipts.iter(),
                self.chain_spec.as_ref(),
                block.timestamp,
            ) {
                debug!(target: "evm", ?error, ?receipts, "receipts verification failed");
                return Err(error);
            };
            self.stats.receipt_root_duration += time.elapsed();
        }

        self.save_receipts(receipts)
    }

    fn execute_transactions(
        &mut self,
        block: &BlockWithSenders,
        total_difficulty: U256,
    ) -> Result<(Vec<Receipt>, u64), BlockExecutionError> {
        self.init_env(&block.header, total_difficulty);

        // perf: do not execute empty blocks
        if block.body.is_empty() {
            return Ok((Vec::new(), 0));
        }

        let is_regolith =
            self.chain_spec.fork(Hardfork::Regolith).active_at_timestamp(block.timestamp);

        // Ensure that the create2deployer is force-deployed at the canyon transition. Optimism
        // blocks will always have at least a single transaction in them (the L1 info transaction),
        // so we can safely assume that this will always be triggered upon the transition and that
        // the above check for empty blocks will never be hit on OP chains.
        super::ensure_create2_deployer(self.chain_spec().clone(), block.timestamp, self.db_mut())
            .map_err(|_| {
            BlockExecutionError::OptimismBlockExecution(
                OptimismBlockExecutionError::ForceCreate2DeployerFail,
            )
        })?;

        let mut cumulative_gas_used = 0;
        let mut receipts = Vec::with_capacity(block.body.len());
        for (sender, transaction) in block.transactions_with_sender() {
            let time = Instant::now();
            // The sum of the transaction’s gas limit, Tg, and the gas utilized in this block prior,
            // must be no greater than the block’s gasLimit.
            let block_available_gas = block.header.gas_limit - cumulative_gas_used;
            if transaction.gas_limit() > block_available_gas
                && (is_regolith || !transaction.is_system_transaction())
            {
                return Err(BlockValidationError::TransactionGasLimitMoreThanAvailableBlockGas {
                    transaction_gas_limit: transaction.gas_limit(),
                    block_available_gas,
                }
                .into());
            }

            // An optimism block should never contain blob transactions.
            if matches!(transaction.tx_type(), TxType::EIP4844) {
                return Err(BlockExecutionError::OptimismBlockExecution(
                    OptimismBlockExecutionError::BlobTransactionRejected,
                ));
            }

            // Cache the depositor account prior to the state transition for the deposit nonce.
            //
            // Note that this *only* needs to be done post-regolith hardfork, as deposit nonces
            // were not introduced in Bedrock. In addition, regular transactions don't have deposit
            // nonces, so we don't need to touch the DB for those.
            let depositor = (is_regolith && transaction.is_deposit())
                .then(|| {
                    self.db_mut()
                        .load_cache_account(*sender)
                        .map(|acc| acc.account_info().unwrap_or_default())
                })
                .transpose()
                .map_err(|_| BlockExecutionError::ProviderError)?;

            // Execute transaction.
            let ResultAndState { result, state } = self.transact(transaction, *sender)?;
            trace!(
                target: "evm",
                ?transaction, ?result, ?state,
                "Executed transaction"
            );
            self.stats.execution_duration += time.elapsed();
            let time = Instant::now();

            self.db_mut().commit(state);

            self.stats.apply_state_duration += time.elapsed();

            // append gas used
            cumulative_gas_used += result.gas_used();

            // Push transaction changeset and calculate header bloom filter for receipt.
            receipts.push(Receipt {
                tx_type: transaction.tx_type(),
                // Success flag was added in `EIP-658: Embedding transaction status code in
                // receipts`.
                success: result.is_success(),
                cumulative_gas_used,
                // convert to reth log
                logs: result.into_logs().into_iter().map(Into::into).collect(),
                #[cfg(feature = "optimism")]
                deposit_nonce: depositor.map(|account| account.nonce),
                // The deposit receipt version was introduced in Canyon to indicate an update to how
                // receipt hashes should be computed when set. The state transition process ensures
                // this is only set for post-Canyon deposit transactions.
                #[cfg(feature = "optimism")]
                deposit_receipt_version: (transaction.is_deposit()
                    && self
                        .chain_spec()
                        .is_fork_active_at_timestamp(Hardfork::Canyon, block.timestamp))
                .then_some(1),
            });
        }

        Ok((receipts, cumulative_gas_used))
    }

    fn take_output_state(&mut self) -> BundleStateWithReceipts {
        let receipts = std::mem::take(&mut self.receipts);
        BundleStateWithReceipts::new(
            self.evm.context.evm.db.take_bundle(),
            receipts,
            self.first_block.unwrap_or_default(),
        )
    }

    fn stats(&self) -> BlockExecutorStats {
        self.stats.clone()
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.evm.context.evm.db.bundle_size_hint())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{database::StateProviderDatabase, test_utils::StateProviderTest};
    use reth_node_optimism::OptimismEvmConfig;
    use reth_primitives::{
        bytes, keccak256, Account, Address, Block, Bytes, ChainSpecBuilder, ForkCondition, Header,
        Signature, StorageKey, StorageValue, Transaction, TransactionKind, TransactionSigned,
        TxEip1559, BASE_MAINNET,
    };
    use revm::L1_BLOCK_CONTRACT;
    use std::{collections::HashMap, str::FromStr, sync::Arc};

    static L1_BLOCK_CONTRACT_CODE: Bytes = bytes!("608060405234801561001057600080fd5b50600436106100c95760003560e01c80638381f58a11610081578063b80777ea1161005b578063b80777ea14610170578063e591b28214610190578063e81b2c6d146101d057600080fd5b80638381f58a1461014a5780638b239f731461015e5780639e8c49661461016757600080fd5b806354fd4d50116100b257806354fd4d50146100ff5780635cf249691461011457806364ca23ef1461011d57600080fd5b8063015d8eb9146100ce57806309bd5a60146100e3575b600080fd5b6100e16100dc366004610515565b6101d9565b005b6100ec60025481565b6040519081526020015b60405180910390f35b610107610318565b6040516100f691906105b7565b6100ec60015481565b6003546101319067ffffffffffffffff1681565b60405167ffffffffffffffff90911681526020016100f6565b6000546101319067ffffffffffffffff1681565b6100ec60055481565b6100ec60065481565b6000546101319068010000000000000000900467ffffffffffffffff1681565b6101ab73deaddeaddeaddeaddeaddeaddeaddeaddead000181565b60405173ffffffffffffffffffffffffffffffffffffffff90911681526020016100f6565b6100ec60045481565b3373deaddeaddeaddeaddeaddeaddeaddeaddead000114610280576040517f08c379a000000000000000000000000000000000000000000000000000000000815260206004820152603b60248201527f4c31426c6f636b3a206f6e6c7920746865206465706f7369746f72206163636f60448201527f756e742063616e20736574204c3120626c6f636b2076616c7565730000000000606482015260840160405180910390fd5b6000805467ffffffffffffffff98891668010000000000000000027fffffffffffffffffffffffffffffffff00000000000000000000000000000000909116998916999099179890981790975560019490945560029290925560038054919094167fffffffffffffffffffffffffffffffffffffffffffffffff00000000000000009190911617909255600491909155600555600655565b60606103437f00000000000000000000000000000000000000000000000000000000000000016103bb565b61036c7f00000000000000000000000000000000000000000000000000000000000000006103bb565b6103957f00000000000000000000000000000000000000000000000000000000000000006103bb565b6040516020016103a793929190610608565b604051602081830303815290604052905090565b6060816000036103fe57505060408051808201909152600181527f3000000000000000000000000000000000000000000000000000000000000000602082015290565b8160005b81156104285780610412816106ad565b91506104219050600a83610714565b9150610402565b60008167ffffffffffffffff81111561044357610443610728565b6040519080825280601f01601f19166020018201604052801561046d576020820181803683370190505b5090505b84156104f057610482600183610757565b915061048f600a8661076e565b61049a906030610782565b60f81b8183815181106104af576104af61079a565b60200101907effffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff1916908160001a9053506104e9600a86610714565b9450610471565b949350505050565b803567ffffffffffffffff8116811461051057600080fd5b919050565b600080600080600080600080610100898b03121561053257600080fd5b61053b896104f8565b975061054960208a016104f8565b9650604089013595506060890135945061056560808a016104f8565b979a969950949793969560a0850135955060c08501359460e001359350915050565b60005b838110156105a257818101518382015260200161058a565b838111156105b1576000848401525b50505050565b60208152600082518060208401526105d6816040850160208701610587565b601f017fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffe0169190910160400192915050565b6000845161061a818460208901610587565b80830190507f2e000000000000000000000000000000000000000000000000000000000000008082528551610656816001850160208a01610587565b60019201918201528351610671816002840160208801610587565b0160020195945050505050565b7f4e487b7100000000000000000000000000000000000000000000000000000000600052601160045260246000fd5b60007fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff82036106de576106de61067e565b5060010190565b7f4e487b7100000000000000000000000000000000000000000000000000000000600052601260045260246000fd5b600082610723576107236106e5565b500490565b7f4e487b7100000000000000000000000000000000000000000000000000000000600052604160045260246000fd5b6000828210156107695761076961067e565b500390565b60008261077d5761077d6106e5565b500690565b600082198211156107955761079561067e565b500190565b7f4e487b7100000000000000000000000000000000000000000000000000000000600052603260045260246000fdfea164736f6c634300080f000a");

    fn create_op_state_provider() -> StateProviderTest {
        let mut db = StateProviderTest::default();

        let l1_block_contract_account = Account {
            balance: U256::ZERO,
            bytecode_hash: Some(keccak256(L1_BLOCK_CONTRACT_CODE.clone())),
            nonce: 1,
        };

        let mut l1_block_storage = HashMap::new();
        // base fee
        l1_block_storage.insert(StorageKey::with_last_byte(1), StorageValue::from(1000000000));
        // l1 fee overhead
        l1_block_storage.insert(StorageKey::with_last_byte(5), StorageValue::from(188));
        // l1 fee scalar
        l1_block_storage.insert(StorageKey::with_last_byte(6), StorageValue::from(684000));
        // l1 free scalars post ecotone
        l1_block_storage.insert(
            StorageKey::with_last_byte(3),
            StorageValue::from_str(
                "0x0000000000000000000000000000000000001db0000d27300000000000000005",
            )
            .unwrap(),
        );

        db.insert_account(
            L1_BLOCK_CONTRACT,
            l1_block_contract_account,
            Some(L1_BLOCK_CONTRACT_CODE.clone()),
            l1_block_storage,
        );

        db
    }

    fn create_op_evm_processor<'a>(
        chain_spec: Arc<ChainSpec>,
        db: StateProviderTest,
    ) -> EVMProcessor<'a, OptimismEvmConfig> {
        let mut executor = EVMProcessor::new_with_db(
            chain_spec,
            StateProviderDatabase::new(db),
            OptimismEvmConfig::default(),
        );
        executor.evm.context.evm.db.load_cache_account(L1_BLOCK_CONTRACT).unwrap();
        executor
    }

    #[test]
    fn canyon_deposit_receipt_version_pre_canyon() {
        let header = Header {
            timestamp: 1,
            number: 1,
            gas_limit: 1_000_000,
            gas_used: 42_000,
            ..Header::default()
        };

        let mut db = create_op_state_provider();
        let addr = Address::ZERO;
        let account = Account { balance: U256::MAX, ..Account::default() };

        db.insert_account(addr, account, None, HashMap::new());

        let chain_spec =
            Arc::new(ChainSpecBuilder::from(&*BASE_MAINNET).regolith_activated().build());

        let tx = TransactionSigned::from_transaction_and_signature(
            Transaction::Eip1559(TxEip1559 {
                chain_id: chain_spec.chain.id(),
                nonce: 0,
                gas_limit: 21_000,
                to: TransactionKind::Call(addr),
                ..Default::default()
            }),
            Signature::default(),
        );

        let tx_deposit = TransactionSigned::from_transaction_and_signature(
            Transaction::Deposit(reth_primitives::TxDeposit {
                from: addr,
                to: TransactionKind::Call(addr),
                gas_limit: 21_000,
                ..Default::default()
            }),
            Signature::default(),
        );

        let mut executor = create_op_evm_processor(chain_spec, db);

        // attempt to execute an empty block with parent beacon block root, this should not fail
        executor
            .execute(
                &BlockWithSenders {
                    block: Block {
                        header: header.clone(),
                        body: vec![tx, tx_deposit],
                        ommers: vec![],
                        withdrawals: None,
                    },
                    senders: vec![addr, addr],
                },
                U256::ZERO,
            )
            .expect("Executing a block while canyon is not active should not fail");

        dbg!(&executor.receipts);

        // non-deposit tx
        assert_eq!(executor.receipts[0][0].as_ref().unwrap().deposit_receipt_version, None);
        // deposit tx
        assert_eq!(executor.receipts[0][1].as_ref().unwrap().deposit_receipt_version, None);
    }

    #[test]
    fn canyon_deposit_receipt_version_post_canyon() {
        // set timestamp >= 2 ensure_create2_deployer
        let header = Header {
            timestamp: 2,
            number: 1,
            gas_limit: 1_000_000,
            gas_used: 42_000,
            ..Header::default()
        };

        let mut db = create_op_state_provider();
        let addr = Address::ZERO;
        let account = Account { balance: U256::MAX, ..Account::default() };

        db.insert_account(addr, account, None, HashMap::new());

        let chain_spec =
            Arc::new(ChainSpecBuilder::from(&*BASE_MAINNET).canyon_activated().build());

        let tx = TransactionSigned::from_transaction_and_signature(
            Transaction::Eip1559(TxEip1559 {
                chain_id: chain_spec.chain.id(),
                nonce: 0,
                gas_limit: 21_000,
                to: TransactionKind::Call(addr),
                ..Default::default()
            }),
            Signature::default(),
        );

        let tx_deposit = TransactionSigned::from_transaction_and_signature(
            Transaction::Deposit(reth_primitives::TxDeposit {
                from: addr,
                to: TransactionKind::Call(addr),
                gas_limit: 21_000,
                ..Default::default()
            }),
            Signature::default(),
        );

        let mut executor = create_op_evm_processor(chain_spec, db);

        // attempt to execute an empty block with parent beacon block root, this should not fail
        executor
            .execute(
                &BlockWithSenders {
                    block: Block {
                        header: header.clone(),
                        body: vec![tx, tx_deposit],
                        ommers: vec![],
                        withdrawals: None,
                    },
                    senders: vec![addr, addr],
                },
                U256::ZERO,
            )
            .expect("Executing a block while canyon is active should not fail");

        // non-deposit tx
        assert_eq!(executor.receipts[0][0].as_ref().unwrap().deposit_receipt_version, None);
        // deposit tx
        assert_eq!(executor.receipts[0][1].as_ref().unwrap().deposit_receipt_version, Some(1));
    }
}
