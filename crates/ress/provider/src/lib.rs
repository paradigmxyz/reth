//! Reth implementation of [`reth_ress_protocol::RessProtocolProvider`].

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use alloy_consensus::BlockHeader as _;
use alloy_primitives::{Bytes, B256};
use alloy_rlp::Encodable;
use parking_lot::Mutex;
use reth_chain_state::{ExecutedBlock, ExecutedBlockWithTrieUpdates, MemoryOverlayStateProvider};
use reth_errors::{ProviderError, ProviderResult};
use reth_ethereum_primitives::{Block, BlockBody, EthPrimitives};
use reth_evm::{execute::Executor, ConfigureEvm};
use reth_primitives_traits::{Block as _, Header, RecoveredBlock};
use reth_provider::{
    BlockReader, BlockSource, ProviderError, ProviderResult, StateProviderFactory,
};
use reth_ress_protocol::{RLPExecutionWitness, RessProtocolProvider};
use reth_revm::{database::StateProviderDatabase, db::State, witness::ExecutionWitnessRecord};
use reth_tasks::TaskSpawner;
use reth_trie::{MultiProofTargets, Nibbles, TrieInput};
use schnellru::{ByLength, LruMap};
use std::{collections::HashSet, sync::Arc, time::Instant};
use tokio::sync::{oneshot, Semaphore};
use tracing::*;

mod recorder;
use recorder::StateWitnessRecorderDatabase;

mod pending_state;
pub use pending_state::*;
use reth_storage_api::{BlockReader, BlockSource, StateProviderFactory};

/// Reth provider implementing [`RessProtocolProvider`].
#[expect(missing_debug_implementations)]
#[derive(Clone)]
pub struct RethRessProtocolProvider<P, E> {
    provider: P,
    evm_config: E,
    task_spawner: Box<dyn TaskSpawner>,
    max_witness_window: u64,
    witness_semaphore: Arc<Semaphore>,
    witness_cache: Arc<Mutex<LruMap<B256, Arc<RLPExecutionWitness>>>>,
    pending_state: PendingState<EthPrimitives>,
}

impl<P, E> RethRessProtocolProvider<P, E>
where
    P: BlockReader<Block = Block> + StateProviderFactory,
    E: ConfigureEvm<Primitives = EthPrimitives> + 'static,
{
    /// Create new ress protocol provider.
    pub fn new(
        provider: P,
        evm_config: E,
        task_spawner: Box<dyn TaskSpawner>,
        // TODO: Add a config to save X amounts of proofs, where X is related to the
        // TODO: reorg depth
        max_witness_window: u64,
        witness_max_parallel: usize,
        cache_size: u32,
        pending_state: PendingState<EthPrimitives>,
    ) -> eyre::Result<Self> {
        Ok(Self {
            provider,
            evm_config,
            task_spawner,
            max_witness_window,
            witness_semaphore: Arc::new(Semaphore::new(witness_max_parallel)),
            witness_cache: Arc::new(Mutex::new(LruMap::new(ByLength::new(cache_size)))),
            pending_state,
        })
    }

    /// Retrieve a valid or invalid block by block hash.
    pub fn block_by_hash(
        &self,
        block_hash: B256,
    ) -> ProviderResult<Option<Arc<RecoveredBlock<Block>>>> {
        // NOTE: we keep track of the pending state locally because reth does not provider a way
        // to access non-canonical or invalid blocks via the provider.
        let maybe_block = if let Some(block) = self.pending_state.recovered_block(&block_hash) {
            Some(block)
        } else if let Some(block) =
            self.provider.find_block_by_hash(block_hash, BlockSource::Any)?
        {
            let signers = block.recover_signers()?;
            Some(Arc::new(block.into_recovered_with_signers(signers)))
        } else {
            // we attempt to look up invalid block last
            self.pending_state.invalid_recovered_block(&block_hash)
        };
        Ok(maybe_block)
    }

    /// Generate state witness
    pub fn generate_witness(&self, block_hash: B256) -> ProviderResult<RLPExecutionWitness> {
        if let Some(witness) = self.witness_cache.lock().get(&block_hash).cloned() {
            return Ok(witness.as_ref().clone());
        }

        let block =
            self.block_by_hash(block_hash)?.ok_or(ProviderError::BlockHashNotFound(block_hash))?;

        let best_block_number = self.provider.best_block_number()?;
        if best_block_number.saturating_sub(block.number()) > self.max_witness_window {
            return Err(ProviderError::TrieWitnessError(
                "witness target block exceeds maximum witness window".to_owned(),
            ));
        }

        let mut executed_ancestors = Vec::new();
        let mut ancestor_hash = block.parent_hash();
        let historical = 'sp: loop {
            match self.provider.state_by_block_hash(ancestor_hash) {
                Ok(state_provider) => break 'sp state_provider,
                Err(_) => {
                    // Attempt to retrieve a valid executed block first.
                    let mut executed = self.pending_state.executed_block(&ancestor_hash);

                    // If it's not present, attempt to lookup invalid block.
                    if executed.is_none() {
                        if let Some(invalid) =
                            self.pending_state.invalid_recovered_block(&ancestor_hash)
                        {
                            trace!(target: "reth::ress_provider", %block_hash, %ancestor_hash, "Using invalid ancestor block for witness construction");
                            executed = Some(ExecutedBlockWithTrieUpdates {
                                block: ExecutedBlock {
                                    recovered_block: invalid,
                                    ..Default::default()
                                },
                                ..Default::default()
                            });
                        }
                    }

                    let Some(executed) = executed else {
                        return Err(ProviderError::StateForHashNotFound(ancestor_hash));
                    };
                    ancestor_hash = executed.sealed_block().parent_hash();
                    executed_ancestors.push(executed);
                }
            };
        };

        // Execute all gathered blocks to gather accesses state.
        let mut db = StateWitnessRecorderDatabase::new(StateProviderDatabase::new(
            MemoryOverlayStateProvider::new(historical, executed_ancestors.clone()),
        ));
        let mut record = ExecutionWitnessRecord::default();

        // We allow block execution to fail, since we still want to record all accessed state by
        // invalid blocks.
        if let Err(error) = self.evm_config.batch_executor(&mut db).execute_with_state_closure(
            &block,
            |state: &State<_>| {
                record.record_executed_state(state);
            },
        ) {
            debug!(target: "reth::ress_provider", %block_hash, %error, "Error executing the block");
        }

        // NOTE: there might be a race condition where target ancestor hash gets evicted from the
        // database.
        let witness_state_provider = self.provider.state_by_block_hash(ancestor_hash)?;
        let mut trie_input = TrieInput::default();
        for block in executed_ancestors.into_iter().rev() {
            trie_input.append_cached_ref(&block.trie, &block.hashed_state);
        }
        let execution_witness_record =
            merge_execution_witness_records(db.execution_witness_record(), record);
        let hashed_state = execution_witness_record.hashed_state;

        // Gather the state witness.
        let witness = if hashed_state.is_empty() {
            // If no state was accessed, at least the root node must be present.
            let multiproof = witness_state_provider.multiproof(
                trie_input,
                MultiProofTargets::from_iter([(B256::ZERO, Default::default())]),
            )?;
            let mut witness = Vec::new();
            if let Some(root_node) =
                multiproof.account_subtree.into_inner().remove(&Nibbles::default())
            {
                witness.push(root_node);
            }
            witness
        } else {
            witness_state_provider.witness(trie_input, hashed_state)?
        };

        // TODO: The code below was partially copied from debug_executionWitness
        let block_number = block.number();
        let smallest = match execution_witness_record.lowest_block_number {
            Some(smallest) => smallest,
            None => {
                // Return only the parent header, if there were no calls to the
                // BLOCKHASH opcode.
                block_number.saturating_sub(1)
            }
        };

        let range = smallest..block_number;
        let headers: Vec<Bytes> = self
            .provider
            .headers_range(range)?
            .into_iter()
            .map(|header| {
                let mut serialized_header = Vec::new();
                header.encode(&mut serialized_header);
                serialized_header.into()
            })
            .collect();
        let execution_witness =
            RLPExecutionWitness { state: witness, codes: execution_witness_record.codes, headers };

        // Insert witness into the cache.
        self.witness_cache.lock().insert(block_hash, Arc::new(execution_witness.clone()));

        Ok(execution_witness)
    }
}

impl<P, E> RessProtocolProvider for RethRessProtocolProvider<P, E>
where
    P: BlockReader<Block = Block> + StateProviderFactory + Clone + 'static,
    E: ConfigureEvm<Primitives = EthPrimitives> + 'static,
{
    fn header(&self, block_hash: B256) -> ProviderResult<Option<Header>> {
        trace!(target: "reth::ress_provider", %block_hash, "Serving header");
        Ok(self.block_by_hash(block_hash)?.map(|b| b.header().clone()))
    }

    fn block_body(&self, block_hash: B256) -> ProviderResult<Option<BlockBody>> {
        trace!(target: "reth::ress_provider", %block_hash, "Serving block body");
        Ok(self.block_by_hash(block_hash)?.map(|b| b.body().clone()))
    }

    fn bytecode(&self, code_hash: B256) -> ProviderResult<Option<Bytes>> {
        trace!(target: "reth::ress_provider", %code_hash, "Serving bytecode");
        let maybe_bytecode = 'bytecode: {
            if let Some(bytecode) = self.pending_state.find_bytecode(code_hash) {
                break 'bytecode Some(bytecode);
            }

            self.provider.latest()?.bytecode_by_hash(&code_hash)?
        };

        Ok(maybe_bytecode.map(|bytecode| bytecode.original_bytes()))
    }

    async fn witness(&self, block_hash: B256) -> ProviderResult<RLPExecutionWitness> {
        trace!(target: "reth::ress_provider", %block_hash, "Serving witness");
        let started_at = Instant::now();
        let _permit = self.witness_semaphore.acquire().await.map_err(ProviderError::other)?;
        let this = self.clone();
        let (tx, rx) = oneshot::channel();
        self.task_spawner.spawn_blocking(Box::pin(async move {
            let result = this.generate_witness(block_hash);
            let _ = tx.send(result);
        }));
        match rx.await {
            Ok(Ok(witness)) => {
                trace!(target: "reth::ress_provider", %block_hash, elapsed = ?started_at.elapsed(), "Computed witness");
                Ok(witness)
            }
            Ok(Err(error)) => Err(error),
            Err(_) => Err(ProviderError::TrieWitnessError("dropped".to_owned())),
        }
    }
}

// TODO: Possibly put as a method on `ExecutionWitnessRecord`
// TODO: Leaving it here to keep the changes in the ress directory
fn merge_execution_witness_records(
    lhs: ExecutionWitnessRecord,
    rhs: ExecutionWitnessRecord,
) -> ExecutionWitnessRecord {
    // Merge the two execution witness records
    //
    // Merge the hashed post state
    let mut hashed_state = lhs.hashed_state;
    hashed_state.extend(rhs.hashed_state);
    //
    // Merge bytecode
    let codes: Vec<_> = {
        let mut merged: HashSet<_> = lhs.codes.into_iter().collect();
        merged.extend(rhs.codes);
        merged.into_iter().collect()
    };
    // Merge lowest accessed block number
    let lowest_block_number = {
        let a = lhs.lowest_block_number;
        let b = rhs.lowest_block_number;
        match (a, b) {
            (Some(a_val), Some(b_val)) => Some(a_val.min(b_val)),
            // Since we know that they are either both `None` or one of them is `None`
            // `or` will return the `Some` value or `None` if they are both `None`
            _ => a.or(b),
        }
    };

    // Note: We do not merge the preimages because this is not useful for us
    ExecutionWitnessRecord { hashed_state, codes, keys: Vec::new(), lowest_block_number }
}
