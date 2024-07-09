use reth_evm::execute::{
    BatchExecutor, BlockExecutionError, BlockExecutionOutput, BlockExecutorProvider, Executor,
};
use reth_node_api::FullNodeComponents;
use reth_primitives::{Block, BlockNumber, BlockWithSenders, Receipt};
use reth_primitives_traits::format_gas_throughput;
use reth_provider::{
    BlockReader, Chain, HeaderProvider, ProviderError, StateProviderFactory, TransactionVariant,
};
use reth_prune_types::PruneModes;
use reth_revm::database::StateProviderDatabase;
use reth_stages_api::ExecutionStageThresholds;
use reth_tracing::tracing::{debug, trace};
use std::{
    ops::RangeInclusive,
    time::{Duration, Instant},
};

/// Factory for creating new backfill jobs.
#[derive(Debug, Clone)]
pub struct BackfillJobFactory<E, P> {
    executor: E,
    provider: P,
    prune_modes: PruneModes,
    thresholds: ExecutionStageThresholds,
}

impl<E, P> BackfillJobFactory<E, P> {
    /// Creates a new [`BackfillJobFactory`].
    pub fn new(executor: E, provider: P) -> Self {
        Self {
            executor,
            provider,
            prune_modes: PruneModes::none(),
            thresholds: ExecutionStageThresholds::default(),
        }
    }

    /// Sets the prune modes
    pub fn with_prune_modes(mut self, prune_modes: PruneModes) -> Self {
        self.prune_modes = prune_modes;
        self
    }

    /// Sets the thresholds
    pub const fn with_thresholds(mut self, thresholds: ExecutionStageThresholds) -> Self {
        self.thresholds = thresholds;
        self
    }
}

impl<E: Clone, P: Clone> BackfillJobFactory<E, P> {
    /// Creates a new backfill job for the given range.
    pub fn backfill(&self, range: RangeInclusive<BlockNumber>) -> BackfillJob<E, P> {
        BackfillJob {
            executor: self.executor.clone(),
            provider: self.provider.clone(),
            prune_modes: self.prune_modes.clone(),
            range,
            thresholds: self.thresholds.clone(),
        }
    }
}

impl BackfillJobFactory<(), ()> {
    /// Creates a new [`BackfillJobFactory`] from [`FullNodeComponents`].
    pub fn new_from_components<Node: FullNodeComponents>(
        components: Node,
    ) -> BackfillJobFactory<Node::Executor, Node::Provider> {
        BackfillJobFactory::<_, _>::new(
            components.block_executor().clone(),
            components.provider().clone(),
        )
    }
}

/// Backfill job started for a specific range.
///
/// It implements [`Iterator`] that executes blocks in batches according to the provided thresholds
/// and yields [`Chain`]
#[derive(Debug)]
pub struct BackfillJob<E, P> {
    executor: E,
    provider: P,
    prune_modes: PruneModes,
    thresholds: ExecutionStageThresholds,
    range: RangeInclusive<BlockNumber>,
}

impl<E, P> Iterator for BackfillJob<E, P>
where
    E: BlockExecutorProvider,
    P: HeaderProvider + BlockReader + StateProviderFactory,
{
    type Item = Result<Chain, BlockExecutionError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.range.is_empty() {
            return None
        }

        Some(self.execute_range())
    }
}

impl<E, P> BackfillJob<E, P>
where
    E: BlockExecutorProvider,
    P: BlockReader + HeaderProvider + StateProviderFactory,
{
    fn execute_range(&mut self) -> Result<Chain, BlockExecutionError> {
        let mut executor = self.executor.batch_executor(StateProviderDatabase::new(
            self.provider.history_by_block_number(self.range.start().saturating_sub(1))?,
        ));
        executor.set_prune_modes(self.prune_modes.clone());

        let mut fetch_block_duration = Duration::default();
        let mut execution_duration = Duration::default();
        let mut cumulative_gas = 0;
        let batch_start = Instant::now();

        let mut blocks = Vec::new();
        for block_number in self.range.clone() {
            // Fetch the block
            let fetch_block_start = Instant::now();

            let td = self
                .provider
                .header_td_by_number(block_number)?
                .ok_or_else(|| ProviderError::HeaderNotFound(block_number.into()))?;

            // we need the block's transactions along with their hashes
            let block = self
                .provider
                .sealed_block_with_senders(block_number.into(), TransactionVariant::WithHash)?
                .ok_or_else(|| ProviderError::HeaderNotFound(block_number.into()))?;

            fetch_block_duration += fetch_block_start.elapsed();

            cumulative_gas += block.gas_used;

            // Configure the executor to use the current state.
            trace!(target: "exex::backfill", number = block_number, txs = block.body.len(), "Executing block");

            // Execute the block
            let execute_start = Instant::now();

            // Unseal the block for execution
            let (block, senders) = block.into_components();
            let (unsealed_header, hash) = block.header.split();
            let block = Block {
                header: unsealed_header,
                body: block.body,
                ommers: block.ommers,
                withdrawals: block.withdrawals,
                requests: block.requests,
            }
            .with_senders_unchecked(senders);

            executor.execute_and_verify_one((&block, td).into())?;
            execution_duration += execute_start.elapsed();

            // TODO(alexey): report gas metrics using `block.header.gas_used`

            // Seal the block back and save it
            blocks.push(block.seal(hash));

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

impl<E, P> BackfillJob<E, P> {
    /// Converts the backfill job into a single block backfill job.
    pub fn into_single_blocks(self) -> SingleBlockBackfillJob<E, P> {
        self.into()
    }
}

impl<E, P> From<BackfillJob<E, P>> for SingleBlockBackfillJob<E, P> {
    fn from(value: BackfillJob<E, P>) -> Self {
        Self { executor: value.executor, provider: value.provider, range: value.range }
    }
}

/// Single block Backfill job started for a specific range.
///
/// It implements [`Iterator`] which executes a block each time the
/// iterator is advanced and yields ([`BlockWithSenders`], [`BlockExecutionOutput`])
#[derive(Debug)]
pub struct SingleBlockBackfillJob<E, P> {
    executor: E,
    provider: P,
    range: RangeInclusive<BlockNumber>,
}

impl<E, P> Iterator for SingleBlockBackfillJob<E, P>
where
    E: BlockExecutorProvider,
    P: HeaderProvider + BlockReader + StateProviderFactory,
{
    type Item = Result<(BlockWithSenders, BlockExecutionOutput<Receipt>), BlockExecutionError>;

    fn next(&mut self) -> Option<Self::Item> {
        self.range.next().map(|block_number| self.execute_block(block_number))
    }
}

impl<E, P> SingleBlockBackfillJob<E, P>
where
    E: BlockExecutorProvider,
    P: HeaderProvider + BlockReader + StateProviderFactory,
{
    fn execute_block(
        &self,
        block_number: u64,
    ) -> Result<(BlockWithSenders, BlockExecutionOutput<Receipt>), BlockExecutionError> {
        let td = self
            .provider
            .header_td_by_number(block_number)?
            .ok_or_else(|| ProviderError::HeaderNotFound(block_number.into()))?;

        // Fetch the block with senders for execution.
        let block_with_senders = self
            .provider
            .block_with_senders(block_number.into(), TransactionVariant::WithHash)?
            .ok_or_else(|| ProviderError::HeaderNotFound(block_number.into()))?;

        // Configure the executor to use the previous block's state.
        let executor = self.executor.executor(StateProviderDatabase::new(
            self.provider.history_by_block_number(block_number.saturating_sub(1))?,
        ));

        trace!(target: "exex::backfill", number = block_number, txs = block_with_senders.block.body.len(), "Executing block");

        let block_execution_output = executor.execute((&block_with_senders, td).into())?;

        Ok((block_with_senders, block_execution_output))
    }
}

#[cfg(test)]
mod tests {
    use crate::BackfillJobFactory;
    use eyre::OptionExt;
    use reth_blockchain_tree::noop::NoopBlockchainTree;
    use reth_chainspec::{ChainSpec, ChainSpecBuilder, EthereumHardfork, MAINNET};
    use reth_db_common::init::init_genesis;
    use reth_evm::execute::{
        BlockExecutionInput, BlockExecutionOutput, BlockExecutorProvider, Executor,
    };
    use reth_evm_ethereum::execute::EthExecutorProvider;
    use reth_primitives::{
        b256, constants::ETH_TO_WEI, public_key_to_address, Address, Block, BlockWithSenders,
        Genesis, GenesisAccount, Header, Receipt, Requests, SealedBlockWithSenders, Transaction,
        TxEip2930, TxKind, U256,
    };
    use reth_provider::{
        providers::BlockchainProvider, test_utils::create_test_provider_factory_with_chain_spec,
        BlockWriter, ExecutionOutcome, LatestStateProviderRef, ProviderFactory,
    };
    use reth_revm::database::StateProviderDatabase;
    use reth_testing_utils::generators::{self, sign_tx_with_key_pair};
    use secp256k1::Keypair;
    use std::sync::Arc;

    fn to_execution_outcome(
        block_number: u64,
        block_execution_output: &BlockExecutionOutput<Receipt>,
    ) -> ExecutionOutcome {
        ExecutionOutcome {
            bundle: block_execution_output.state.clone(),
            receipts: block_execution_output.receipts.clone().into(),
            first_block: block_number,
            requests: vec![Requests(block_execution_output.requests.clone())],
        }
    }

    fn chain_spec(address: Address) -> Arc<ChainSpec> {
        // Create a chain spec with a genesis state that contains the
        // provided sender
        Arc::new(
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
        )
    }

    fn execute_block_and_commit_to_database<DB>(
        provider_factory: &ProviderFactory<DB>,
        chain_spec: Arc<ChainSpec>,
        block: &BlockWithSenders,
    ) -> eyre::Result<BlockExecutionOutput<Receipt>>
    where
        DB: reth_db_api::database::Database,
    {
        let provider = provider_factory.provider()?;

        // Execute the block to produce a block execution output
        let mut block_execution_output = EthExecutorProvider::ethereum(chain_spec)
            .executor(StateProviderDatabase::new(LatestStateProviderRef::new(
                provider.tx_ref(),
                provider.static_file_provider().clone(),
            )))
            .execute(BlockExecutionInput { block, total_difficulty: U256::ZERO })?;
        block_execution_output.state.reverts.sort();

        // Convert the block execution output to an execution outcome for committing to the database
        let execution_outcome = to_execution_outcome(block.number, &block_execution_output);

        // Commit the block's execution outcome to the database
        let provider_rw = provider_factory.provider_rw()?;
        let block = block.clone().seal_slow();
        provider_rw.append_blocks_with_state(
            vec![block],
            execution_outcome,
            Default::default(),
            Default::default(),
        )?;
        provider_rw.commit()?;

        Ok(block_execution_output)
    }

    fn blocks_and_execution_outputs<DB>(
        provider_factory: ProviderFactory<DB>,
        chain_spec: Arc<ChainSpec>,
        key_pair: Keypair,
    ) -> eyre::Result<Vec<(SealedBlockWithSenders, BlockExecutionOutput<Receipt>)>>
    where
        DB: reth_db_api::database::Database,
    {
        // First block has a transaction that transfers some ETH to zero address
        let block1 = Block {
            header: Header {
                parent_hash: chain_spec.genesis_hash(),
                receipts_root: b256!(
                    "d3a6acf9a244d78b33831df95d472c4128ea85bf079a1d41e32ed0b7d2244c9e"
                ),
                difficulty: chain_spec.fork(EthereumHardfork::Paris).ttd().expect("Paris TTD"),
                number: 1,
                gas_limit: 21000,
                gas_used: 21000,
                ..Default::default()
            },
            body: vec![sign_tx_with_key_pair(
                key_pair,
                Transaction::Eip2930(TxEip2930 {
                    chain_id: chain_spec.chain.id(),
                    nonce: 0,
                    gas_limit: 21000,
                    gas_price: 1_500_000_000,
                    to: TxKind::Call(Address::ZERO),
                    value: U256::from(0.1 * ETH_TO_WEI as f64),
                    ..Default::default()
                }),
            )],
            ..Default::default()
        }
        .with_recovered_senders()
        .ok_or_eyre("failed to recover senders")?;

        // Second block resends the same transaction with increased nonce
        let block2 = Block {
            header: Header {
                parent_hash: block1.header.hash_slow(),
                receipts_root: b256!(
                    "d3a6acf9a244d78b33831df95d472c4128ea85bf079a1d41e32ed0b7d2244c9e"
                ),
                difficulty: chain_spec.fork(EthereumHardfork::Paris).ttd().expect("Paris TTD"),
                number: 2,
                gas_limit: 21000,
                gas_used: 21000,
                ..Default::default()
            },
            body: vec![sign_tx_with_key_pair(
                key_pair,
                Transaction::Eip2930(TxEip2930 {
                    chain_id: chain_spec.chain.id(),
                    nonce: 1,
                    gas_limit: 21000,
                    gas_price: 1_500_000_000,
                    to: TxKind::Call(Address::ZERO),
                    value: U256::from(0.1 * ETH_TO_WEI as f64),
                    ..Default::default()
                }),
            )],
            ..Default::default()
        }
        .with_recovered_senders()
        .ok_or_eyre("failed to recover senders")?;

        let block_output1 =
            execute_block_and_commit_to_database(&provider_factory, chain_spec.clone(), &block1)?;
        let block_output2 =
            execute_block_and_commit_to_database(&provider_factory, chain_spec, &block2)?;

        let block1 = block1.seal_slow();
        let block2 = block2.seal_slow();

        Ok(vec![(block1, block_output1), (block2, block_output2)])
    }

    #[test]
    fn test_backfill() -> eyre::Result<()> {
        reth_tracing::init_test_tracing();

        // Create a key pair for the sender
        let key_pair = Keypair::new_global(&mut generators::rng());
        let address = public_key_to_address(key_pair.public_key());

        let chain_spec = chain_spec(address);

        let executor = EthExecutorProvider::ethereum(chain_spec.clone());
        let provider_factory = create_test_provider_factory_with_chain_spec(chain_spec.clone());
        init_genesis(provider_factory.clone())?;
        let blockchain_db = BlockchainProvider::new(
            provider_factory.clone(),
            Arc::new(NoopBlockchainTree::default()),
        )?;

        let blocks_and_execution_outputs =
            blocks_and_execution_outputs(provider_factory, chain_spec, key_pair)?;
        let (block, block_execution_output) = blocks_and_execution_outputs.first().unwrap();
        let execution_outcome = to_execution_outcome(block.number, block_execution_output);

        // Backfill the first block
        let factory = BackfillJobFactory::new(executor, blockchain_db);
        let job = factory.backfill(1..=1);
        let chains = job.collect::<Result<Vec<_>, _>>()?;

        // Assert that the backfill job produced the same chain as we got before when we were
        // executing only the first block
        assert_eq!(chains.len(), 1);
        let mut chain = chains.into_iter().next().unwrap();
        chain.execution_outcome_mut().bundle.reverts.sort();
        assert_eq!(chain.blocks(), &[(1, block.clone())].into());
        assert_eq!(chain.execution_outcome(), &execution_outcome);

        Ok(())
    }

    #[test]
    fn test_single_block_backfill() -> eyre::Result<()> {
        reth_tracing::init_test_tracing();

        // Create a key pair for the sender
        let key_pair = Keypair::new_global(&mut generators::rng());
        let address = public_key_to_address(key_pair.public_key());

        let chain_spec = chain_spec(address);

        let executor = EthExecutorProvider::ethereum(chain_spec.clone());
        let provider_factory = create_test_provider_factory_with_chain_spec(chain_spec.clone());
        init_genesis(provider_factory.clone())?;
        let blockchain_db = BlockchainProvider::new(
            provider_factory.clone(),
            Arc::new(NoopBlockchainTree::default()),
        )?;

        let blocks_and_execution_outcomes =
            blocks_and_execution_outputs(provider_factory, chain_spec, key_pair)?;

        // Backfill the first block
        let factory = BackfillJobFactory::new(executor, blockchain_db);
        let job = factory.backfill(1..=1);
        let single_job = job.into_single_blocks();
        let block_execution_it = single_job.into_iter();

        // Assert that the backfill job only produces a single block
        let blocks_and_outcomes = block_execution_it.collect::<Vec<_>>();
        assert_eq!(blocks_and_outcomes.len(), 1);

        // Assert that the backfill job single block iterator produces the expected output for each
        // block
        for (i, res) in blocks_and_outcomes.into_iter().enumerate() {
            let (block, mut execution_output) = res?;
            execution_output.state.reverts.sort();

            let sealed_block_with_senders = blocks_and_execution_outcomes[i].0.clone();
            let expected_block = sealed_block_with_senders.unseal();
            let expected_output = &blocks_and_execution_outcomes[i].1;

            assert_eq!(block, expected_block);
            assert_eq!(&execution_output, expected_output);
        }

        Ok(())
    }
}
