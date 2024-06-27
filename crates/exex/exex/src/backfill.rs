use reth_evm::execute::{BatchExecutor, BlockExecutionError, BlockExecutorProvider};
use reth_node_api::FullNodeComponents;
use reth_node_core::node_config::NodeConfig;
use reth_primitives::BlockNumber;
use reth_provider::{
    BlockReader, Chain, DatabaseProviderFactory, HeaderProvider, HistoricalStateProviderRef,
    ProviderError, StaticFileProviderFactory, TransactionVariant,
};
use reth_prune_types::PruneModes;
use reth_revm::database::StateProviderDatabase;
use reth_stages_api::format_gas_throughput;
use reth_stages_types::ExecutionStageThresholds;
use reth_tracing::tracing::{debug, trace};
use std::{
    ops::RangeInclusive,
    time::{Duration, Instant},
};

/// Factory for creating new backfill jobs.
#[derive(Debug, Clone)]
pub struct BackfillJobFactory<Node: FullNodeComponents> {
    components: Node,
    config: NodeConfig,
    thresholds: ExecutionStageThresholds,
}

impl<Node: FullNodeComponents> BackfillJobFactory<Node> {
    /// Creates a new backfill job factory.
    pub const fn new(
        components: Node,
        config: NodeConfig,
        thresholds: ExecutionStageThresholds,
    ) -> Self {
        Self { components, config, thresholds }
    }

    /// Creates a new backfill job for the given range.
    pub fn backfill(&self, range: RangeInclusive<BlockNumber>) -> BackfillJob<Node> {
        BackfillJob {
            components: self.components.clone(),
            prune_modes: self.config.prune_config().unwrap_or_default().segments,
            range,
            thresholds: self.thresholds.clone(),
        }
    }
}

/// Backfill job started for a specific range.
///
/// It implements [`Iterator`] that executes blocks in batches and yields
/// [notifications](`crate::ExExNotification)s.
#[derive(Debug)]
pub struct BackfillJob<Node: FullNodeComponents> {
    components: Node,
    prune_modes: PruneModes,
    range: RangeInclusive<BlockNumber>,
    thresholds: ExecutionStageThresholds,
}

impl<Node: FullNodeComponents> Iterator for BackfillJob<Node> {
    type Item = Result<Chain, BlockExecutionError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.range.is_empty() {
            return None
        }

        Some(self.execute_range())
    }
}

impl<Node: FullNodeComponents> BackfillJob<Node> {
    fn execute_range(&mut self) -> Result<Chain, BlockExecutionError> {
        let provider = self.components.provider();
        let provider_ro = provider.database_provider_ro()?.disable_long_read_transaction_safety();

        let mut executor = self.components.block_executor().batch_executor(
            StateProviderDatabase(HistoricalStateProviderRef::new(
                provider_ro.tx_ref(),
                *self.range.start(),
                provider.static_file_provider(),
            )),
            self.prune_modes.clone(),
        );

        let mut fetch_block_duration = Duration::default();
        let mut execution_duration = Duration::default();
        let mut cumulative_gas = 0;
        let batch_start = Instant::now();

        let mut blocks = Vec::new();
        for block_number in self.range.clone() {
            // Fetch the block
            let fetch_block_start = Instant::now();

            let td = provider
                .header_td_by_number(block_number)?
                .ok_or_else(|| ProviderError::HeaderNotFound(block_number.into()))?;

            // we need the block's transactions but we don't need the transaction hashes
            let block = provider
                .block_with_senders(block_number.into(), TransactionVariant::NoHash)?
                .ok_or_else(|| ProviderError::HeaderNotFound(block_number.into()))?;

            fetch_block_duration += fetch_block_start.elapsed();

            cumulative_gas += block.gas_used;

            // Configure the executor to use the current state.
            trace!(target: "exex::backfill", number = block_number, txs = block.body.len(), "Executing block");

            // Execute the block
            let execute_start = Instant::now();

            executor.execute_and_verify_one((&block, td).into())?;
            execution_duration += execute_start.elapsed();

            // TODO(alexey): report gas metrics using `block.header.gas_used`

            blocks.push(block.seal_slow());

            // Check if we should commit now
            let bundle_size_hint = executor.size_hint().unwrap_or_default() as u64;
            if self.thresholds.is_end_of_batch(
                block_number - *self.range.start(),
                bundle_size_hint,
                cumulative_gas,
                batch_start.elapsed(),
            ) {
                break
            }
        }

        let last_block_number = blocks.last().expect("blocks should not be empty").number;
        debug!(
            target: "exex::backfill",
            range = ?*self.range.start()..=last_block_number,
            block_fetch = ?fetch_block_duration,
            execution = ?execution_duration,
            throughput = format_gas_throughput(cumulative_gas, execution_duration),
            "Finished executing block range"
        );
        self.range = last_block_number + 1..=*self.range.end();

        let chain = Chain::new(blocks, executor.finalize(), None);
        Ok(chain)
    }
}

#[cfg(test)]
mod tests {
    use crate::BackfillJobFactory;
    use eyre::OptionExt;
    use reth_chainspec::{ChainSpecBuilder, Hardfork, MAINNET};
    use reth_consensus::test_utils::TestConsensus;
    use reth_evm::execute::{BatchExecutor, BlockExecutorProvider};
    use reth_evm_ethereum::execute::EthExecutorProvider;
    use reth_exex_test_utils::{test_exex_context_with_chain_spec, TestNode};
    use reth_node_api::FullNodeTypesAdapter;
    use reth_node_builder::{components::Components, NodeAdapter};
    use reth_primitives::{
        constants::ETH_TO_WEI, public_key_to_address, Block, Genesis, GenesisAccount, Header, U256,
    };
    use reth_provider::{BlockWriter, LatestStateProviderRef, OriginalValuesKnown, StateWriter};
    use reth_prune_types::PruneModes;
    use reth_revm::database::StateProviderDatabase;
    use reth_testing_utils::generators;
    use secp256k1::Keypair;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_backfill() -> eyre::Result<()> {
        let key_pair = Keypair::new_global(&mut generators::rng());
        let address = public_key_to_address(key_pair.public_key());

        let chain_spec = Arc::new(
            ChainSpecBuilder::default()
                .chain(MAINNET.chain)
                .genesis(Genesis {
                    alloc: [(
                        address,
                        GenesisAccount { balance: U256::from(ETH_TO_WEI), ..Default::default() },
                    )]
                    .into(),
                    ..MAINNET.genesis.clone()
                })
                .paris_activated()
                .build(),
        );
        let (ctx, handle) = test_exex_context_with_chain_spec(chain_spec.clone()).await?;
        let components = NodeAdapter::<FullNodeTypesAdapter<TestNode, _, _>, _> {
            components: Components {
                transaction_pool: ctx.pool().clone(),
                evm_config: *ctx.evm_config(),
                executor: EthExecutorProvider::ethereum(chain_spec.clone()),
                consensus: Arc::new(TestConsensus::default()),
                network: ctx.network().clone(),
                payload_builder: ctx.payload_builder().clone(),
            },
            task_executor: handle.tasks.executor(),
            provider: ctx.provider().clone(),
        };

        let block1 = Block {
            header: Header {
                parent_hash: chain_spec.genesis_hash(),
                difficulty: chain_spec.fork(Hardfork::Paris).ttd().expect("Paris TTD"),
                number: 1,
                ..Default::default()
            },
            body: vec![],
            ..Default::default()
        }
        .with_recovered_senders()
        .ok_or_eyre("failed to recover senders")?;

        let block2 = Block {
            header: Header {
                parent_hash: block1.hash_slow(),
                difficulty: chain_spec.fork(Hardfork::Paris).ttd().expect("Paris TTD"),
                number: 2,
                ..Default::default()
            },
            body: vec![],
            ..Default::default()
        }
        .with_recovered_senders()
        .ok_or_eyre("failed to recover senders")?;

        let provider = handle.provider_factory.provider()?;

        let outcome_single = EthExecutorProvider::ethereum(chain_spec.clone())
            .batch_executor(
                StateProviderDatabase::new(LatestStateProviderRef::new(
                    provider.tx_ref(),
                    provider.static_file_provider().clone(),
                )),
                PruneModes::none(),
            )
            .execute_and_verify_batch([(&block1, U256::ZERO).into()])?;

        let outcome_batch = EthExecutorProvider::ethereum(chain_spec.clone())
            .batch_executor(
                StateProviderDatabase::new(LatestStateProviderRef::new(
                    provider.tx_ref(),
                    provider.static_file_provider().clone(),
                )),
                PruneModes::none(),
            )
            .execute_and_verify_batch([
                (&block1, U256::ZERO).into(),
                (&block2, U256::ZERO).into(),
            ])?;

        let block1 = block1.seal_slow();
        let block2 = block2.seal_slow();

        let provider_rw = handle.provider_factory.provider_rw()?;
        provider_rw.insert_block(block1.clone(), None)?;
        provider_rw.insert_block(block2, None)?;
        outcome_batch.write_to_storage(provider_rw.tx_ref(), None, OriginalValuesKnown::Yes)?;
        provider_rw.commit()?;

        let factory = BackfillJobFactory::new(components, ctx.config, Default::default());

        let job = factory.backfill(1..=1);
        let chains = job.collect::<Result<Vec<_>, _>>()?;
        assert_eq!(chains.len(), 1);

        let chain = chains.first().unwrap();
        assert_eq!(chain.blocks(), &[(1, block1)].into());
        assert_eq!(chain.execution_outcome(), &outcome_single);

        Ok(())
    }
}
