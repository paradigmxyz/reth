use reth_config::config::{EtlConfig, HashingConfig};
use reth_stages_api::{
    ExecInput, ExecOutput, Stage, StageCheckpoint, StageError, StageId, UnwindInput, UnwindOutput,
};
use std::fmt::Debug;

/// Account hashing stage.
///
/// This stage is now a no-op because hashing is done during execution - state is written
/// directly to hashed tables (`HashedAccounts`, `HashedStorages`) during block execution.
#[derive(Clone, Debug, Default)]
pub struct AccountHashingStage {
    /// The threshold (in number of blocks) for switching between incremental
    /// hashing and full storage hashing.
    pub clean_threshold: u64,
    /// The maximum number of accounts to process before committing during unwind.
    pub commit_threshold: u64,
    /// ETL configuration
    pub etl_config: EtlConfig,
}

impl AccountHashingStage {
    /// Create new instance of [`AccountHashingStage`].
    pub const fn new(config: HashingConfig, etl_config: EtlConfig) -> Self {
        Self {
            clean_threshold: config.clean_threshold,
            commit_threshold: config.commit_threshold,
            etl_config,
        }
    }
}

impl<Provider> Stage<Provider> for AccountHashingStage {
    fn id(&self) -> StageId {
        StageId::AccountHashing
    }

    fn execute(&mut self, _provider: &Provider, input: ExecInput) -> Result<ExecOutput, StageError> {
        Ok(ExecOutput::done(StageCheckpoint::new(input.target())))
    }

    fn unwind(
        &mut self,
        _provider: &Provider,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        Ok(UnwindOutput { checkpoint: StageCheckpoint::new(input.unwind_to) })
    }
}

#[cfg(any(test, feature = "test-utils"))]
mod test_utils {
    use super::*;
    use alloy_primitives::B256;
    use reth_db_api::transaction::{DbTx, DbTxMut};
    use reth_primitives_traits::Account;
    use reth_stages_api::StageError;
    use std::ops::{Range, RangeInclusive};

    impl AccountHashingStage {
        /// Initializes the `HashedAccounts` table with `num_accounts` having some random state
        /// at the target block, with `txs_range` transactions in each block.
        pub fn seed<Tx: DbTx + DbTxMut + 'static, N: reth_provider::providers::ProviderNodeTypes>(
            provider: &reth_provider::DatabaseProvider<Tx, N>,
            opts: SeedOpts,
        ) -> Result<Vec<(alloy_primitives::Address, Account)>, StageError>
        where
            N::Primitives: reth_primitives_traits::NodePrimitives<
                Block = reth_ethereum_primitives::Block,
                BlockHeader = reth_primitives_traits::Header,
            >,
        {
            use alloy_primitives::U256;
            use reth_db_api::{cursor::DbCursorRW, models::AccountBeforeTx, tables};
            use reth_provider::{BlockWriter, StaticFileProviderFactory, StaticFileWriter};
            use reth_testing_utils::{
                generators,
                generators::{random_block_range, random_eoa_accounts, BlockRangeParams},
            };

            let mut rng = generators::rng();

            let blocks = random_block_range(
                &mut rng,
                opts.blocks.clone(),
                BlockRangeParams {
                    parent: Some(B256::ZERO),
                    tx_count: opts.txs,
                    ..Default::default()
                },
            );

            for block in blocks {
                provider.insert_block(&block.try_recover().unwrap()).unwrap();
            }
            provider
                .static_file_provider()
                .latest_writer(reth_static_file_types::StaticFileSegment::Headers)
                .unwrap()
                .commit()
                .unwrap();
            let mut accounts = random_eoa_accounts(&mut rng, opts.accounts);
            {
                let mut account_cursor =
                    provider.tx_ref().cursor_write::<reth_db_api::tables::HashedAccounts>()?;
                accounts.sort_by_key(|a| alloy_primitives::keccak256(a.0));
                for (addr, acc) in &accounts {
                    account_cursor.append(alloy_primitives::keccak256(addr), acc)?;
                }

                let mut acc_changeset_cursor =
                    provider.tx_ref().cursor_write::<tables::AccountChangeSets>()?;
                for (t, (addr, acc)) in opts.blocks.zip(&accounts) {
                    let Account { nonce, balance, .. } = acc;
                    let prev_acc = Account {
                        nonce: nonce - 1,
                        balance: balance - U256::from(1),
                        bytecode_hash: None,
                    };
                    let hashed_address = alloy_primitives::keccak256(addr);
                    let acc_before_tx =
                        AccountBeforeTx { hashed_address, info: Some(prev_acc) };
                    acc_changeset_cursor.append(t, &acc_before_tx)?;
                }
            }

            Ok(accounts)
        }
    }

    /// `SeedOpts` provides configuration parameters for calling `AccountHashingStage::seed`
    /// in unit tests or benchmarks to generate an initial database state for running the
    /// stage.
    #[derive(Clone, Debug)]
    pub struct SeedOpts {
        /// The range of blocks to be generated
        pub blocks: RangeInclusive<u64>,
        /// The number of accounts to be generated
        pub accounts: usize,
        /// The range of transactions to be generated per block.
        pub txs: Range<u8>,
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use test_utils::*;
