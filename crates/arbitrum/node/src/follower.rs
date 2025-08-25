use std::sync::{Arc, OnceLock};
use alloy_primitives::{B256, Address, U256};
use eyre::Result;
use alloy_rpc_types_engine::ForkchoiceState;
use alloy_rpc_types_engine::PayloadAttributes;
use core::future::Future;
use core::pin::Pin;
use reth_node_api::ConsensusEngineHandle;
use reth_provider::StateProviderFactory;

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
}

pub type DynFollowerExecutor = Arc<dyn FollowerExecutor>;

static FOLLOWER_EXECUTOR: OnceLock<DynFollowerExecutor> = OnceLock::new();

pub fn set_follower_executor(exec: DynFollowerExecutor) {
    let _ = FOLLOWER_EXECUTOR.set(exec);
}

pub fn get_follower_executor() -> Option<DynFollowerExecutor> {
    FOLLOWER_EXECUTOR.get().cloned()
}

#[derive(Clone)]
pub struct FollowerExecutorHandle {
    pub provider: Arc<dyn StateProviderFactory + Send + Sync>,
    pub beacon: ConsensusEngineHandle<crate::engine::ArbEngineTypes<reth_arbitrum_payload::ArbPayloadTypes>>,
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
}
