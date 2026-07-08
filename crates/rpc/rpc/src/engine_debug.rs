//! Debug RPC helpers backed by the consensus engine handle.

use alloy_primitives::U64;
use alloy_rpc_types_engine::ForkchoiceState;
use jsonrpsee::{core::RpcResult, RpcModule};
use reth_engine_primitives::ConsensusEngineHandle;
use reth_payload_primitives::PayloadTypes;
use reth_rpc_server_types::result::{internal_rpc_err, invalid_params_rpc_err, ToRpcResult};
use reth_storage_api::HeaderProvider;

/// Returns a `debug_setHead` RPC module backed by the consensus engine.
pub fn debug_set_head_rpc_module<Provider, Payload>(
    provider: Provider,
    engine_handle: ConsensusEngineHandle<Payload>,
) -> RpcModule<()>
where
    Provider: HeaderProvider + Clone + Send + Sync + 'static,
    Payload: PayloadTypes,
{
    let mut module = RpcModule::new(());
    module
        .register_async_method("debug_setHead", move |params, _, _| {
            let provider = provider.clone();
            let engine_handle = engine_handle.clone();
            async move {
                let res: RpcResult<()> = async {
                    let number = params.one::<U64>()?.to::<u64>();
                    let header =
                        provider.sealed_header(number).to_rpc_result()?.ok_or_else(|| {
                            invalid_params_rpc_err(format!("block {number} not found"))
                        })?;

                    let state = ForkchoiceState {
                        head_block_hash: header.hash(),
                        safe_block_hash: header.hash(),
                        finalized_block_hash: header.hash(),
                    };
                    let fcu = engine_handle
                        .fork_choice_updated_with_canonical_head_unwind(state, None)
                        .await
                        .map_err(|err| internal_rpc_err(err.to_string()))?;
                    if !fcu.is_valid() {
                        return Err(invalid_params_rpc_err(format!(
                            "forkchoice update returned non-valid status: {:?}",
                            fcu.payload_status.status
                        )))
                    }

                    Ok(())
                }
                .await;
                res
            }
        })
        .expect("valid method name");
    module
}
