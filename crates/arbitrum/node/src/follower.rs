use std::sync::{Arc, OnceLock};
use alloy_primitives::{B256, Address, U256};
use eyre::Result;
use alloy_rpc_types_engine::ForkchoiceState;
use alloy_rpc_types_engine::PayloadAttributes;
use core::future::Future;
use core::pin::Pin;
use reth_node_api::ConsensusEngineHandle;
use reth_provider::{StateProviderFactory, BlockNumReader, BlockHashReader};

pub trait FollowerExecutor: Send + Sync {
    fn execute_message_to_block(
        &self,
        parent_hash: B256,
        attrs: PayloadAttributes,
        l2msg_bytes: &[u8],
        poster: Address,
        request_id: Option<B256>,
        kind: u8,
        l1_block_number: u64,
        delayed_messages_read: u64,
        l1_base_fee: U256,
        batch_gas_cost: Option<u64>,
    ) -> Pin<Box<dyn Future<Output = Result<(B256, B256)>> + Send + '_>>;

    /// Get the current canonical head block number and hash from reth's database.
    /// Returns None if no canonical head exists (empty database).
    fn canonical_head(&self) -> Pin<Box<dyn Future<Output = Result<Option<(u64, B256)>>> + Send + '_>>;

    /// Get the genesis block number for this chain.
    fn genesis_block_number(&self) -> u64;

    /// Get the block result (hash and send_root) for a block that already exists in reth.
    /// Returns None if the block doesn't exist.
    /// Returns (block_hash, send_root) tuple.
    fn block_result_at(&self, block_number: u64) -> Pin<Box<dyn Future<Output = Result<Option<(B256, B256)>>> + Send + '_>>;
}

pub type DynFollowerExecutor = Arc<dyn FollowerExecutor>;

static FOLLOWER_EXECUTOR: OnceLock<DynFollowerExecutor> = OnceLock::new();

pub fn set_follower_executor(exec: DynFollowerExecutor) {
    let _ = FOLLOWER_EXECUTOR.set(exec);
}

pub fn get_follower_executor() -> Option<DynFollowerExecutor> {
    FOLLOWER_EXECUTOR.get().cloned()
}

/// Combined trait for providers that support both state and block info queries
pub trait CanonicalChainProvider: StateProviderFactory + BlockNumReader + BlockHashReader + Send + Sync {}
impl<T: StateProviderFactory + BlockNumReader + BlockHashReader + Send + Sync> CanonicalChainProvider for T {}

#[derive(Clone)]
pub struct FollowerExecutorHandle {
    pub provider: Arc<dyn CanonicalChainProvider>,
    pub beacon: ConsensusEngineHandle<crate::engine::ArbEngineTypes<reth_arbitrum_payload::ArbPayloadTypes>>,
    /// Genesis block number for this chain (from ArbitrumChainParams.GenesisBlockNum)
    /// Used for BlockNumberToMessageIndex: messageIndex = blockNum - genesis
    pub genesis_block_number: u64,
}

impl FollowerExecutor for FollowerExecutorHandle {
    fn execute_message_to_block(
        &self,
        parent_hash: B256,
        attrs: PayloadAttributes,
        l2msg_bytes: &[u8],
        poster: Address,
        request_id: Option<B256>,
        kind: u8,
        l1_block_number: u64,
        delayed_messages_read: u64,
        l1_base_fee: U256,
        batch_gas_cost: Option<u64>,
    ) -> Pin<Box<dyn Future<Output = Result<(B256, B256)>> + Send>> {
        let beacon = self.beacon.clone();
        let l2_owned: Vec<u8> = l2msg_bytes.to_vec();
        Box::pin(async move {
            let _ = beacon
                .fork_choice_updated(
                    ForkchoiceState {
                        head_block_hash: parent_hash,
                        safe_block_hash: parent_hash,
                        finalized_block_hash: parent_hash,
                    },
                    None,
                    reth_payload_primitives::EngineApiMessageVersion::default(),
                )
                .await?;

            if let Some(exec) = get_follower_executor() {
                return exec
                    .execute_message_to_block(
                        parent_hash,
                        attrs,
                        &l2_owned,
                        poster,
                        request_id,
                        kind,
                        l1_block_number,
                        delayed_messages_read,
                        l1_base_fee,
                        batch_gas_cost,
                    )
                    .await;
            }

            eyre::bail!("no follower executor registered")
        })
    }

    fn canonical_head(&self) -> Pin<Box<dyn Future<Output = Result<Option<(u64, B256)>>> + Send + '_>> {
        let provider = self.provider.clone();
        Box::pin(async move {
            // Get the best (canonical) block number from reth's database
            let block_number = match provider.best_block_number() {
                Ok(n) => n,
                Err(_) => return Ok(None),
            };

            // Get the block hash for that block number
            let block_hash = match provider.block_hash(block_number) {
                Ok(Some(hash)) => hash,
                Ok(None) => return Ok(None),
                Err(_) => return Ok(None),
            };

            Ok(Some((block_number, block_hash)))
        })
    }

    fn genesis_block_number(&self) -> u64 {
        self.genesis_block_number
    }

    fn block_result_at(&self, block_number: u64) -> Pin<Box<dyn Future<Output = Result<Option<(B256, B256)>>> + Send + '_>> {
        Box::pin(async move {
            // Delegate to the real follower executor if available
            if let Some(exec) = get_follower_executor() {
                return exec.block_result_at(block_number).await;
            }

            // Fallback: just get block hash, send_root will be zero
            // This handles the case where the real executor isn't registered yet
            let block_hash = match self.provider.block_hash(block_number) {
                Ok(Some(hash)) => hash,
                Ok(None) => return Ok(None),
                Err(_) => return Ok(None),
            };

            // Can't get send_root without HeaderProvider, return zero
            // The caller should cache the real result from execution
            Ok(Some((block_hash, B256::ZERO)))
        })
    }
}
