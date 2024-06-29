use reth_db_api::database::Database;
use reth_evm::execute::{BatchExecutor, BlockExecutionError, BlockExecutorProvider};
use reth_node_api::FullNodeComponents;
use reth_primitives::{Block, BlockNumber};
use reth_provider::{Chain, FullProvider, ProviderError, TransactionVariant};
use reth_prune_types::PruneModes;
use reth_revm::database::StateProviderDatabase;
use reth_stages_api::{format_gas_throughput, ExecutionStageThresholds};
use reth_tracing::tracing::{debug, trace};
use std::{
    marker::PhantomData,
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
    pub fn new(executor: E, provider: P, prune_modes: PruneModes) -> Self {
        Self { executor, provider, prune_modes, thresholds: ExecutionStageThresholds::default() }
    }

    /// Sets the thresholds
    pub const fn with_thresholds(mut self, thresholds: ExecutionStageThresholds) -> Self {
        self.thresholds = thresholds;
        self
    }
}

impl<E: Clone, P: Clone> BackfillJobFactory<E, P> {
    /// Creates a new backfill job for the given range.
    pub fn backfill<DB>(&self, range: RangeInclusive<BlockNumber>) -> BackfillJob<E, DB, P> {
        BackfillJob {
            executor: self.executor.clone(),
            provider: self.provider.clone(),
            prune_modes: self.prune_modes.clone(),
            range,
            thresholds: self.thresholds.clone(),
            _db: PhantomData,
        }
    }
}

impl BackfillJobFactory<(), ()> {
    /// Creates a new [`BackfillJobFactory`] from [`FullNodeComponents`].
    pub fn new_from_components<Node: FullNodeComponents>(
        components: Node,
        prune_modes: PruneModes,
    ) -> BackfillJobFactory<Node::Executor, Node::Provider> {
        BackfillJobFactory::<_, _>::new(
            components.block_executor().clone(),
            components.provider().clone(),
            prune_modes,
        )
    }
}

/// Backfill job started for a specific range.
///
/// It implements [`Iterator`] that executes blocks in batches according to the provided thresholds
/// and yields [`Chain`]
#[derive(Debug)]
pub struct BackfillJob<E, DB, P> {
    executor: E,
    provider: P,
    prune_modes: PruneModes,
    range: RangeInclusive<BlockNumber>,
    thresholds: ExecutionStageThresholds,
    _db: PhantomData<DB>,
}

impl<E, DB, P> Iterator for BackfillJob<E, DB, P>
where
    E: BlockExecutorProvider,
    DB: Database,
    P: FullProvider<DB>,
{
    type Item = Result<Chain, BlockExecutionError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.range.is_empty() {
            return None
        }

        Some(self.execute_range())
    }
}

impl<E, DB, P> BackfillJob<E, DB, P>
where
    E: BlockExecutorProvider,
    DB: Database,
    P: FullProvider<DB>,
{
    fn execute_range(&mut self) -> Result<Chain, BlockExecutionError> {
        let mut executor = self.executor.batch_executor(
            StateProviderDatabase::new(
                self.provider.history_by_block_number(self.range.start().saturating_sub(1))?,
            ),
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

#[cfg(test)]
mod tests {
    use crate::BackfillJobFactory;
    use eyre::OptionExt;
    use reth_blockchain_tree::noop::NoopBlockchainTree;
    use reth_chainspec::{ChainSpecBuilder, EthereumHardfork, MAINNET};
    use reth_db_common::init::init_genesis;
    use reth_evm::execute::{BatchExecutor, BlockExecutorProvider};
    use reth_evm_ethereum::execute::EthExecutorProvider;
    use reth_primitives::{
        b256, constants::ETH_TO_WEI, public_key_to_address, Address, Block, Genesis,
        GenesisAccount, Header, Transaction, TxEip2930, TxKind, U256,
    };
    use reth_provider::{
        providers::BlockchainProvider, test_utils::create_test_provider_factory_with_chain_spec,
        BlockWriter, LatestStateProviderRef,
    };
    use reth_prune_types::PruneModes;
    use reth_revm::database::StateProviderDatabase;
    use reth_testing_utils::generators::{self, sign_tx_with_key_pair};
    use secp256k1::Keypair;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_backfill() -> eyre::Result<()> {
        reth_tracing::init_test_tracing();

        // Create a key pair for the sender
        let key_pair = Keypair::new_global(&mut generators::rng());
        let address = public_key_to_address(key_pair.public_key());

        // Create a chain spec with a genesis state that contains the sender
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

        let executor = EthExecutorProvider::ethereum(chain_spec.clone());
        let provider_factory = create_test_provider_factory_with_chain_spec(chain_spec.clone());
        init_genesis(provider_factory.clone())?;
        let blockchain_db = BlockchainProvider::new(
            provider_factory.clone(),
            Arc::new(NoopBlockchainTree::default()),
        )?;

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

        // Second block has no state changes
        let block2 = Block {
            header: Header {
                parent_hash: block1.hash_slow(),
                difficulty: chain_spec.fork(EthereumHardfork::Paris).ttd().expect("Paris TTD"),
                number: 2,
                ..Default::default()
            },
            ..Default::default()
        }
        .with_recovered_senders()
        .ok_or_eyre("failed to recover senders")?;

        let provider = provider_factory.provider()?;
        // Execute only the first block on top of genesis state
        let mut outcome_single = EthExecutorProvider::ethereum(chain_spec.clone())
            .batch_executor(
                StateProviderDatabase::new(LatestStateProviderRef::new(
                    provider.tx_ref(),
                    provider.static_file_provider().clone(),
                )),
                PruneModes::none(),
            )
            .execute_and_verify_batch([(&block1, U256::ZERO).into()])?;
        outcome_single.bundle.reverts.sort();
        // Execute both blocks on top of the genesis state
        let outcome_batch = EthExecutorProvider::ethereum(chain_spec)
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
        drop(provider);

        let block1 = block1.seal_slow();
        let block2 = block2.seal_slow();

        // Update the state with the execution results of both blocks
        let provider_rw = provider_factory.provider_rw()?;
        provider_rw.append_blocks_with_state(
            vec![block1.clone(), block2],
            outcome_batch,
            Default::default(),
            Default::default(),
            None,
        )?;
        provider_rw.commit()?;

        // Backfill the first block
        let factory = BackfillJobFactory::new(executor, blockchain_db, PruneModes::none());
        let job = factory.backfill(1..=1);
        let chains = job.collect::<Result<Vec<_>, _>>()?;

        // Assert that the backfill job produced the same chain as we got before when we were
        // executing only the first block
        assert_eq!(chains.len(), 1);
        let mut chain = chains.into_iter().next().unwrap();
        chain.execution_outcome_mut().bundle.reverts.sort();
        assert_eq!(chain.blocks(), &[(1, block1)].into());
        assert_eq!(chain.execution_outcome(), &outcome_single);

        Ok(())
    }
}
