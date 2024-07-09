use clap::Parser;
use eyre::Result;
use tracing::{debug, info};
use crate::commands::common::{AccessRights, Environment, EnvironmentArgs};
use reth_db::{init_db, DatabaseEnv, tables, transaction::DbTx};
use reth_primitives::{BlockNumber, Header};
use std::sync::Arc;
use std::path::{Path, PathBuf};
use reth_blockchain_tree::{BlockchainTree, BlockchainTreeConfig, ShareableBlockchainTree, TreeExternals};
use reth_blockchain_tree::noop::NoopBlockchainTree;
use reth_consensus::Consensus;
use reth_db_api::database::Database;
use reth_evm::execute::{BatchExecutor, BlockExecutionOutput, BlockExecutorProvider, Executor};
use reth_execution_types::ExecutionOutcome;
use reth_provider::{BlockReader, ChainSpecProvider, HeaderProvider, HistoricalStateProviderRef, LatestStateProviderRef, StaticFileProviderFactory};
use reth_provider::providers::BlockchainProvider;
use reth_prune_types::PruneModes;
use reth_revm::database::StateProviderDatabase;
use crate::beacon_consensus::EthBeaconConsensus;
use crate::macros::block_executor;
use crate::primitives::{BlockHashOrNumber, U256};
use crate::providers::{BlockNumReader, OriginalValuesKnown, ProviderError, StateProviderFactory, TransactionVariant};

/// EVM commands
#[derive(Debug, Parser)]
pub struct EvmCommand {
    #[command(flatten)]
    env: EnvironmentArgs,
    /// The block number to fetch the header for
    block_number: BlockHashOrNumber,
}

impl EvmCommand {
    /// Execute the `evm` command
    pub async fn execute(self) -> Result<()> {
        info!(target: "reth::cli", "Executing EVM command...");

        let Environment { provider_factory, config, data_dir } = self.env.init(AccessRights::RO)?;

        let consensus: Arc<dyn Consensus> =
            Arc::new(EthBeaconConsensus::new(provider_factory.chain_spec()));

        let executor = block_executor!(provider_factory.chain_spec());

        // configure blockchain tree
        let tree_externals =
            TreeExternals::new(provider_factory.clone(), Arc::clone(&consensus), executor);
        let tree = BlockchainTree::new(tree_externals, BlockchainTreeConfig::default(), None)?;
        let blockchain_tree = Arc::new(ShareableBlockchainTree::new(tree));
        let blockchain_db = BlockchainProvider::new(provider_factory.clone(), blockchain_tree.clone())?;

        let provider = provider_factory.provider()?;

        let last = provider.last_block_number()?;
        let target = match self.block_number {
                BlockHashOrNumber::Hash(hash) => provider
                    .block_number(hash)?
                    .ok_or_else(|| eyre::eyre!("Block hash not found in database: {hash:?}"))?,
                BlockHashOrNumber::Number(num) => num,
            };

        if target > last {
            eyre::bail!("Target block number is higher than the latest block number")
        }


        let td = provider.header_td_by_number(target)?
            .ok_or_else(|| ProviderError::HeaderNotFound(target.into()))?;

        let block =  provider.sealed_block_with_senders(target.into(), TransactionVariant::WithHash)?
            .ok_or_else(|| ProviderError::HeaderNotFound(target.into()))?;


        let db = StateProviderDatabase::new(blockchain_db.history_by_block_number(target-1)?);
        let executor = block_executor!(provider_factory.chain_spec()).executor(db);

        let BlockExecutionOutput { state, receipts, requests, .. } =
            executor.execute((&block.clone().unseal(),td).into())?;

        let execution_outcome = ExecutionOutcome::new(
            state,
            receipts.into(),
            block.number,
            vec![requests.into()],
        );

        debug!(target: "reth::cli", ?execution_outcome, "Executed block");

        let hashed_post_state = execution_outcome.hash_state_slow();
        let state_root = execution_outcome
            .hash_state_slow()
            .state_root(provider_factory.provider()?.tx_ref())?;

        if state_root != block.state_root {
            eyre::bail!(
                        "state root mismatch. expected: {}. got: {}",
                        block.state_root,
                        state_root
                    );
        }

        Ok(())
    }
}
