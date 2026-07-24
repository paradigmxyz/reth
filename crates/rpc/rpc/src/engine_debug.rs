//! Debug RPC helpers backed by the consensus engine handle.

use alloy_primitives::U64;
use jsonrpsee::{core::RpcResult, RpcModule};
use reth_engine_primitives::{ConsensusEngineHandle, DebugSetHeadError};
use reth_payload_primitives::PayloadTypes;
use reth_rpc_server_types::result::{internal_rpc_err, invalid_params_rpc_err};

/// Returns a `debug_setHead` RPC module backed by the consensus engine.
pub fn debug_set_head_rpc_module<Payload>(
    engine_handle: ConsensusEngineHandle<Payload>,
) -> RpcModule<()>
where
    Payload: PayloadTypes,
{
    let mut module = RpcModule::new(());
    module
        .register_async_method("debug_setHead", move |params, _, _| {
            let engine_handle = engine_handle.clone();
            async move {
                let res: RpcResult<()> = async {
                    let number = params.one::<U64>()?.to::<u64>();
                    if let Err(err) = engine_handle.debug_set_head(number).await {
                        return Err(match err {
                            DebugSetHeadError::BlockNotFound(_) |
                            DebugSetHeadError::Finalized { .. } |
                            DebugSetHeadError::AboveHead { .. } => {
                                invalid_params_rpc_err(err.to_string())
                            }
                            DebugSetHeadError::EngineUnavailable |
                            DebugSetHeadError::Syncing |
                            DebugSetHeadError::Internal(_) => internal_rpc_err(err.to_string()),
                        })
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
