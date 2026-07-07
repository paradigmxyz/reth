//! Debug RPC helpers backed by the canonical chain tracker.

use alloy_primitives::U64;
use alloy_rpc_types_engine::ForkchoiceState;
use jsonrpsee::{core::RpcResult, RpcModule};
use reth_rpc_server_types::result::{invalid_params_rpc_err, ToRpcResult};
use reth_storage_api::{CanonChainTracker, HeaderProvider, ProviderHeader};

/// Returns a `debug_setHead` RPC module backed by the canonical chain tracker.
pub fn debug_set_head_rpc_module<Provider>(provider: Provider) -> RpcModule<()>
where
    Provider: HeaderProvider
        + CanonChainTracker<Header = ProviderHeader<Provider>>
        + Clone
        + Send
        + Sync
        + 'static,
{
    let mut module = RpcModule::new(());
    module
        .register_async_method("debug_setHead", move |params, _, _| {
            let provider = provider.clone();
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
                    provider.on_forkchoice_update_received(&state);
                    provider.set_canonical_head(header.clone());
                    provider.set_safe(header.clone());
                    provider.set_finalized(header);

                    Ok(())
                }
                .await;
                res
            }
        })
        .expect("valid method name");
    module
}
