//! Debug RPC helpers backed by the engine.

use alloy_primitives::U64;
use alloy_rpc_types_engine::ForkchoiceState;
use jsonrpsee::RpcModule;
use reth_engine_primitives::ConsensusEngineHandle;
use reth_payload_primitives::PayloadTypes;
use reth_rpc_server_types::result::{internal_rpc_err, invalid_params_rpc_err, ToRpcResult};
use reth_storage_api::BlockHashReader;

/// Returns a `debug_setHead` RPC module backed by forkchoice updates.
pub fn debug_set_head_rpc_module<Provider, Payload>(
    provider: Provider,
    engine_handle: ConsensusEngineHandle<Payload>,
) -> RpcModule<()>
where
    Provider: BlockHashReader + Clone + Send + Sync + 'static,
    Payload: PayloadTypes,
{
    let mut module = RpcModule::new(());
    module
        .register_async_method("debug_setHead", move |params, _, _| {
            let provider = provider.clone();
            let engine_handle = engine_handle.clone();
            async move {
                let number = params.one::<U64>()?.to::<u64>();
                let block_hash = provider
                    .block_hash(number)
                    .to_rpc_result()?
                    .ok_or_else(|| invalid_params_rpc_err(format!("block {number} not found")))?;

                let state = ForkchoiceState {
                    head_block_hash: block_hash,
                    safe_block_hash: block_hash,
                    finalized_block_hash: block_hash,
                };
                let response = engine_handle
                    .fork_choice_updated_with_canonical_head_unwind(state, None)
                    .await
                    .map_err(|err| internal_rpc_err(err.to_string()))?;
                if response.is_valid() {
                    Ok(())
                } else {
                    Err(internal_rpc_err(format!(
                        "forkchoice update returned non-valid status: {:?}",
                        response.payload_status.status
                    )))
                }
            }
        })
        .expect("valid method name");
    module
}
