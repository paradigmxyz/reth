//! Command for debugging block building.
use crate::macros::block_executor;
use alloy_rlp::Decodable;
use clap::Parser;
use eyre::Context;
use reth_basic_payload_builder::{
    BuildArguments, BuildOutcome, Cancelled, PayloadBuilder, PayloadConfig,
};
use reth_beacon_consensus::EthBeaconConsensus;
use reth_blockchain_tree::{
    BlockchainTree, BlockchainTreeConfig, ShareableBlockchainTree, TreeExternals,
};
use reth_cli_commands::common::{AccessRights, Environment, EnvironmentArgs};
use reth_cli_runner::CliContext;
use reth_consensus::Consensus;
use reth_db::DatabaseEnv;
use reth_errors::RethResult;
use reth_evm::execute::{BlockExecutionOutput, BlockExecutorProvider, Executor};
use reth_execution_types::ExecutionOutcome;
use reth_fs_util as fs;
use reth_node_api::PayloadBuilderAttributes;
use reth_payload_builder::database::CachedReads;
use reth_primitives::{
    constants::eip4844::LoadKzgSettingsError, revm_primitives::KzgSettings, Address,
    BlobTransaction, BlobTransactionSidecar, Bytes, PooledTransactionsElement, SealedBlock,
    SealedBlockWithSenders, Transaction, TransactionSigned, TxEip4844, B256, U256,
};
use reth_provider::{
    providers::BlockchainProvider, BlockHashReader, BlockReader, BlockWriter, ChainSpecProvider,
    ProviderFactory, StageCheckpointReader, StateProviderFactory,
};
use reth_prune::PruneModes;
use reth_revm::{database::StateProviderDatabase, primitives::EnvKzgSettings};
use reth_rpc_types::engine::{BlobsBundleV1, PayloadAttributes};
use reth_stages::StageId;
use reth_transaction_pool::{
    blobstore::InMemoryBlobStore, BlobStore, EthPooledTransaction, PoolConfig, TransactionOrigin,
    TransactionPool, TransactionValidationTaskExecutor,
};
use std::{path::PathBuf, str::FromStr, sync::Arc};
use tracing::*;

/// `reth debug build-block` command
/// This debug routine requires that the node is positioned at the block before the target.
/// The script will then parse the block and attempt to build a similar one.
#[derive(Debug, Parser)]
pub struct Command {
    #[command(flatten)]
    env: EnvironmentArgs,

    /// Overrides the KZG trusted setup by reading from the supplied file.
    #[arg(long, value_name = "PATH")]
    trusted_setup_file: Option<PathBuf>,

    #[arg(long)]
    parent_beacon_block_root: Option<B256>,

    #[arg(long)]
    prev_randao: B256,

    #[arg(long)]
    timestamp: u64,

    #[arg(long)]
    suggested_fee_recipient: Address,

    /// Array of transactions.
    /// NOTE: 4844 transactions must be provided in the same order as they appear in the blobs
    /// bundle.
    #[arg(long, value_delimiter = ',')]
    transactions: Vec<String>,

    /// Path to the file that contains a corresponding blobs bundle.
    #[arg(long)]
    blobs_bundle_path: Option<PathBuf>,
}

impl Command {
    /// Fetches the best block block from the database.
    ///
    /// If the database is empty, returns the genesis block.
    fn lookup_best_block(
        &self,
        factory: ProviderFactory<Arc<DatabaseEnv>>,
    ) -> RethResult<Arc<SealedBlock>> {
        let provider = factory.provider()?;

        let best_number =
            provider.get_stage_checkpoint(StageId::Finish)?.unwrap_or_default().block_number;
        let best_hash = provider
            .block_hash(best_number)?
            .expect("the hash for the latest block is missing, database is corrupt");

        Ok(Arc::new(
            provider
                .block(best_number.into())?
                .expect("the header for the latest block is missing, database is corrupt")
                .seal(best_hash),
        ))
    }

    /// Loads the trusted setup params from a given file path or falls back to
    /// `EnvKzgSettings::Default`.
    fn kzg_settings(&self) -> eyre::Result<EnvKzgSettings> {
        if let Some(ref trusted_setup_file) = self.trusted_setup_file {
            let trusted_setup = KzgSettings::load_trusted_setup_file(trusted_setup_file)
                .map_err(LoadKzgSettingsError::KzgError)?;
            Ok(EnvKzgSettings::Custom(Arc::new(trusted_setup)))
        } else {
            Ok(EnvKzgSettings::Default)
        }
    }

    /// Execute `debug in-memory-merkle` command
    pub async fn execute(self, ctx: CliContext) -> eyre::Result<()> {
        let Environment { provider_factory, .. } = self.env.init(AccessRights::RW)?;

        let consensus: Arc<dyn Consensus> =
            Arc::new(EthBeaconConsensus::new(provider_factory.chain_spec()));

        let executor = block_executor!(provider_factory.chain_spec());

        // configure blockchain tree
        let tree_externals =
            TreeExternals::new(provider_factory.clone(), Arc::clone(&consensus), executor);
        let tree = BlockchainTree::new(
            tree_externals,
            BlockchainTreeConfig::default(),
            PruneModes::none(),
        )?;
        let blockchain_tree = Arc::new(ShareableBlockchainTree::new(tree));

        // fetch the best block from the database
        let best_block = self
            .lookup_best_block(provider_factory.clone())
            .wrap_err("the head block is missing")?;

        let blockchain_db =
            BlockchainProvider::new(provider_factory.clone(), blockchain_tree.clone())?;
        let blob_store = InMemoryBlobStore::default();

        let validator =
            TransactionValidationTaskExecutor::eth_builder(provider_factory.chain_spec())
                .with_head_timestamp(best_block.timestamp)
                .kzg_settings(self.kzg_settings()?)
                .with_additional_tasks(1)
                .build_with_tasks(
                    blockchain_db.clone(),
                    ctx.task_executor.clone(),
                    blob_store.clone(),
                );

        let transaction_pool = reth_transaction_pool::Pool::eth_pool(
            validator,
            blob_store.clone(),
            PoolConfig::default(),
        );
        info!(target: "reth::cli", "Transaction pool initialized");

        let mut blobs_bundle = self
            .blobs_bundle_path
            .map(|path| -> eyre::Result<BlobsBundleV1> {
                let contents = fs::read_to_string(&path)
                    .wrap_err(format!("could not read {}", path.display()))?;
                serde_json::from_str(&contents).wrap_err("failed to deserialize blobs bundle")
            })
            .transpose()?;

        for tx_bytes in &self.transactions {
            debug!(target: "reth::cli", bytes = ?tx_bytes, "Decoding transaction");
            let transaction = TransactionSigned::decode(&mut &Bytes::from_str(tx_bytes)?[..])?
                .into_ecrecovered()
                .ok_or(eyre::eyre!("failed to recover tx"))?;

            let encoded_length = match &transaction.transaction {
                Transaction::Eip4844(TxEip4844 { blob_versioned_hashes, .. }) => {
                    let blobs_bundle = blobs_bundle.as_mut().ok_or(eyre::eyre!(
                        "encountered a blob tx. `--blobs-bundle-path` must be provided"
                    ))?;

                    let sidecar: BlobTransactionSidecar =
                        blobs_bundle.pop_sidecar(blob_versioned_hashes.len());

                    // first construct the tx, calculating the length of the tx with sidecar before
                    // insertion
                    let tx = BlobTransaction::try_from_signed(
                        transaction.as_ref().clone(),
                        sidecar.clone(),
                    )
                    .expect("should not fail to convert blob tx if it is already eip4844");
                    let pooled = PooledTransactionsElement::BlobTransaction(tx);
                    let encoded_length = pooled.length_without_header();

                    // insert the blob into the store
                    blob_store.insert(transaction.hash, sidecar)?;

                    encoded_length
                }
                _ => transaction.length_without_header(),
            };

            debug!(target: "reth::cli", ?transaction, "Adding transaction to the pool");
            transaction_pool
                .add_transaction(
                    TransactionOrigin::External,
                    EthPooledTransaction::new(transaction, encoded_length),
                )
                .await?;
        }

        let payload_attrs = PayloadAttributes {
            parent_beacon_block_root: self.parent_beacon_block_root,
            prev_randao: self.prev_randao,
            timestamp: self.timestamp,
            suggested_fee_recipient: self.suggested_fee_recipient,
            // TODO: add support for withdrawals
            withdrawals: None,
        };
        let payload_config = PayloadConfig::new(
            Arc::clone(&best_block),
            Bytes::default(),
            #[cfg(feature = "optimism")]
            reth_node_optimism::OptimismPayloadBuilderAttributes::try_new(
                best_block.hash(),
                reth_rpc_types::engine::OptimismPayloadAttributes {
                    payload_attributes: payload_attrs,
                    transactions: None,
                    no_tx_pool: None,
                    gas_limit: None,
                },
            )?,
            #[cfg(not(feature = "optimism"))]
            reth_payload_builder::EthPayloadBuilderAttributes::try_new(
                best_block.hash(),
                payload_attrs,
            )?,
            provider_factory.chain_spec(),
        );

        let args = BuildArguments::new(
            blockchain_db.clone(),
            transaction_pool,
            CachedReads::default(),
            payload_config,
            Cancelled::default(),
            None,
        );

        #[cfg(feature = "optimism")]
        let payload_builder = reth_node_optimism::OptimismPayloadBuilder::new(
            reth_node_optimism::OptimismEvmConfig::default(),
        )
        .compute_pending_block();

        #[cfg(not(feature = "optimism"))]
        let payload_builder = reth_ethereum_payload_builder::EthereumPayloadBuilder::default();

        match payload_builder.try_build(args)? {
            BuildOutcome::Better { payload, .. } => {
                let block = payload.block();
                debug!(target: "reth::cli", ?block, "Built new payload");

                consensus.validate_header_with_total_difficulty(block, U256::MAX)?;
                consensus.validate_header(block)?;
                consensus.validate_block_pre_execution(block)?;

                let senders = block.senders().expect("sender recovery failed");
                let block_with_senders =
                    SealedBlockWithSenders::new(block.clone(), senders).unwrap();

                let db = StateProviderDatabase::new(blockchain_db.latest()?);
                let executor = block_executor!(provider_factory.chain_spec()).executor(db);

                let BlockExecutionOutput { state, receipts, requests, .. } =
                    executor.execute((&block_with_senders.clone().unseal(), U256::MAX).into())?;
                let execution_outcome = ExecutionOutcome::new(
                    state,
                    receipts.into(),
                    block.number,
                    vec![requests.into()],
                );

                debug!(target: "reth::cli", ?execution_outcome, "Executed block");

                let hashed_post_state = execution_outcome.hash_state_slow();
                let (state_root, trie_updates) = hashed_post_state
                    .state_root_with_updates(provider_factory.provider()?.tx_ref())?;

                if state_root != block_with_senders.state_root {
                    eyre::bail!(
                        "state root mismatch. expected: {}. got: {}",
                        block_with_senders.state_root,
                        state_root
                    );
                }

                // Attempt to insert new block without committing
                let provider_rw = provider_factory.provider_rw()?;
                provider_rw.append_blocks_with_state(
                    Vec::from([block_with_senders]),
                    execution_outcome,
                    hashed_post_state.into_sorted(),
                    trie_updates,
                )?;
                info!(target: "reth::cli", "Successfully appended built block");
            }
            _ => unreachable!("other outcomes are unreachable"),
        };

        Ok(())
    }
}
