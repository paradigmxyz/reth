use crate::processor::{compare_receipts_root_and_logs_bloom, EVMProcessor};
use reth_interfaces::executor::{
    BlockExecutionError, BlockValidationError, OptimismBlockExecutionError,
};
use reth_node_api::ConfigureEvm;
use reth_primitives::{
    proofs::calculate_receipt_root_optimism, revm_primitives::ResultAndState, BlockWithSenders,
    Bloom, ChainSpec, Hardfork, Receipt, ReceiptWithBloom, TxType, B256, U256,
};
use reth_provider::{BlockExecutor, BundleStateWithReceipts};
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
    EvmConfig: ConfigureEvm,
{
    type Error = BlockExecutionError;

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
                debug!(target: "evm", %error, ?receipts, "receipts verification failed");
                return Err(error)
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
            return Ok((Vec::new(), 0))
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
            if transaction.gas_limit() > block_available_gas &&
                (is_regolith || !transaction.is_system_transaction())
            {
                return Err(BlockValidationError::TransactionGasLimitMoreThanAvailableBlockGas {
                    transaction_gas_limit: transaction.gas_limit(),
                    block_available_gas,
                }
                .into())
            }

            // An optimism block should never contain blob transactions.
            if matches!(transaction.tx_type(), TxType::Eip4844) {
                return Err(BlockExecutionError::OptimismBlockExecution(
                    OptimismBlockExecutionError::BlobTransactionRejected,
                ))
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
                deposit_receipt_version: (transaction.is_deposit() &&
                    self.chain_spec()
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

    fn size_hint(&self) -> Option<usize> {
        Some(self.evm.context.evm.db.bundle_size_hint())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        database::StateProviderDatabase,
        test_utils::{StateProviderTest, TestEvmConfig},
    };
    use reth_primitives::{
        Account, Address, Block, ChainSpecBuilder, Header, Signature, StorageKey, StorageValue,
        Transaction, TransactionKind, TransactionSigned, TxEip1559, BASE_MAINNET,
    };
    use revm::L1_BLOCK_CONTRACT;
    use std::{collections::HashMap, str::FromStr, sync::Arc};

    fn create_op_state_provider() -> StateProviderTest {
        let mut db = StateProviderTest::default();

        let l1_block_contract_account =
            Account { balance: U256::ZERO, bytecode_hash: None, nonce: 1 };

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

        db.insert_account(L1_BLOCK_CONTRACT, l1_block_contract_account, None, l1_block_storage);

        db
    }

    fn create_op_evm_processor<'a>(
        chain_spec: Arc<ChainSpec>,
        db: StateProviderTest,
    ) -> EVMProcessor<'a, TestEvmConfig> {
        let mut executor = EVMProcessor::new_with_db(
            chain_spec,
            StateProviderDatabase::new(db),
            TestEvmConfig::default(),
        );
        executor.evm.context.evm.db.load_cache_account(L1_BLOCK_CONTRACT).unwrap();
        executor
    }

    #[test]
    fn op_deposit_fields_pre_canyon() {
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

        // Attempt to execute a block with one deposit and one non-deposit transaction
        executor
            .execute(
                &BlockWithSenders {
                    block: Block {
                        header,
                        body: vec![tx, tx_deposit],
                        ommers: vec![],
                        withdrawals: None,
                    },
                    senders: vec![addr, addr],
                },
                U256::ZERO,
            )
            .unwrap();

        let tx_receipt = executor.receipts[0][0].as_ref().unwrap();
        let deposit_receipt = executor.receipts[0][1].as_ref().unwrap();

        // deposit_receipt_version is not present in pre canyon transactions
        assert!(deposit_receipt.deposit_receipt_version.is_none());
        assert!(tx_receipt.deposit_receipt_version.is_none());

        // deposit_nonce is present only in deposit transactions
        assert!(deposit_receipt.deposit_nonce.is_some());
        assert!(tx_receipt.deposit_nonce.is_none());
    }

    #[test]
    fn op_deposit_fields_post_canyon() {
        // ensure_create2_deployer will fail if timestamp is set to less then 2
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
            Signature::optimism_deposit_tx_signature(),
        );

        let mut executor = create_op_evm_processor(chain_spec, db);

        // attempt to execute an empty block with parent beacon block root, this should not fail
        executor
            .execute(
                &BlockWithSenders {
                    block: Block {
                        header,
                        body: vec![tx, tx_deposit],
                        ommers: vec![],
                        withdrawals: None,
                    },
                    senders: vec![addr, addr],
                },
                U256::ZERO,
            )
            .expect("Executing a block while canyon is active should not fail");

        let tx_receipt = executor.receipts[0][0].as_ref().unwrap();
        let deposit_receipt = executor.receipts[0][1].as_ref().unwrap();

        // deposit_receipt_version is set to 1 for post canyon deposit transactions
        assert_eq!(deposit_receipt.deposit_receipt_version, Some(1));
        assert!(tx_receipt.deposit_receipt_version.is_none());

        // deposit_nonce is present only in deposit transactions
        assert!(deposit_receipt.deposit_nonce.is_some());
        assert!(tx_receipt.deposit_nonce.is_none());
    }
}
