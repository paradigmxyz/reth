//! Serves the block and receipt methods for recent blocks from their serialized results.
//!
//! `latest` and the few blocks below it are by far the most requested blocks, and every request
//! converted and serialized the same block again. The serialized results are kept per block hash
//! in the [`BlockResponseCache`](reth_rpc_eth_types::BlockResponseCache) of the eth state cache,
//! so they never need updating: the response for a hash cannot change, and the state cache removes
//! the blocks of a reorg.

use alloy_consensus::BlockHeader;
use alloy_eips::{BlockId, BlockNumberOrTag};
use alloy_primitives::B256;
use futures::FutureExt;
use jsonrpsee::{
    core::server::{MethodCallback, MethodResponse, RpcModule},
    types::{Params, ResponsePayload},
};
use reth_rpc_eth_api::{
    helpers::{EthBlocks, LoadBlock},
    FullEthApiServer,
};
use reth_rpc_eth_types::BlockResponseKind;
use reth_storage_api::{BlockNumReader, BlockReaderIdExt};
use serde::Serialize;
use serde_json::value::RawValue;
use std::sync::Arc;

/// Serves `eth_getBlockByNumber`, `eth_getBlockByHash` and `eth_getBlockReceipts` for recent
/// blocks from the serialized results in the state cache of `eth_api`.
///
/// Wraps the methods already registered in `module`. Whatever the cache does not serve — params it
/// cannot parse, `pending` and `earliest`, unknown and older blocks, errors — goes to the original
/// handler with the original params, so those answers are unchanged. Does nothing for a method that
/// is not registered or whose handler is not async, and nothing at all if the cache is disabled.
pub fn cache_block_responses<Ctx, Eth>(module: &mut RpcModule<Ctx>, eth_api: Eth)
where
    Ctx: Send + Sync + 'static,
    Eth: FullEthApiServer + Clone,
{
    if !eth_api.cache().block_responses().is_enabled() {
        return
    }
    wrap(module, "eth_getBlockByNumber", &eth_api, |params| {
        let (number, full) = params.parse::<(BlockNumberOrTag, bool)>().ok()?;
        Some((number.into(), BlockResponseKind::block(full)))
    });
    wrap(module, "eth_getBlockByHash", &eth_api, |params| {
        let (hash, full) = params.parse::<(B256, bool)>().ok()?;
        Some((hash.into(), BlockResponseKind::block(full)))
    });
    wrap(module, "eth_getBlockReceipts", &eth_api, |params| {
        let (block_id,) = params.parse::<(BlockId,)>().ok()?;
        Some((block_id, BlockResponseKind::Receipts))
    });
}

/// Replaces the registered handler of `name` with one that consults the cache first.
///
/// `request` extracts the block and the kind of response from the params, or returns `None` to
/// leave the call to the original handler.
fn wrap<Ctx, Eth>(
    module: &mut RpcModule<Ctx>,
    name: &'static str,
    eth_api: &Eth,
    request: fn(&Params<'_>) -> Option<(BlockId, BlockResponseKind)>,
) where
    Ctx: Send + Sync + 'static,
    Eth: FullEthApiServer + Clone,
{
    // Looked up before it is removed, so a handler that cannot be wrapped stays registered.
    let Some(MethodCallback::Async(original)) = module.method(name).cloned() else { return };
    let eth_api = eth_api.clone();
    let callback =
        MethodCallback::Async(Arc::new(move |id, params, conn_id, max_response_size| {
            let (eth_api, original) = (eth_api.clone(), original.clone());
            async move {
                let cached = match request(&params) {
                    Some((block_id, kind)) => serve(&eth_api, block_id, kind).await,
                    None => None,
                };
                match cached {
                    Some(result) => respond(id, result, max_response_size),
                    None => original(id, params, conn_id, max_response_size).await,
                }
            }
            .boxed()
        }));
    module.remove_method(name);
    module.verify_and_insert(name, callback).expect("the method was just removed");
}

/// Returns the serialized result for `block_id`, computing and caching it on a miss, or `None` if
/// the block is not one the cache serves.
async fn serve<Eth>(eth_api: &Eth, block_id: BlockId, kind: BlockResponseKind) -> Option<SharedJson>
where
    Eth: FullEthApiServer,
{
    let cache = eth_api.cache().block_responses();
    let best = eth_api.provider().best_block_number().ok()?;
    let hash = recent_block_hash(eth_api, block_id, best)?;
    if let Some(result) = cache.get(hash, kind, best) {
        return Some(SharedJson(result));
    }

    // Fetch by the resolved hash rather than the requested tag or number: `latest` may have
    // moved on since it was resolved, and the result must belong to the hash it is stored
    // under.
    let by_hash = BlockId::from(hash);
    let block = LoadBlock::recovered_block(eth_api, by_hash).await.ok()??;
    if !cache.is_recent(block.number(), best) {
        return None;
    }
    let raw = match kind {
        BlockResponseKind::FullBlock | BlockResponseKind::HashesBlock => {
            let full = kind == BlockResponseKind::FullBlock;
            let rpc_block = EthBlocks::rpc_block(eth_api, by_hash, full).await.ok()??;
            serde_json::value::to_raw_value(&rpc_block).ok()?
        }
        BlockResponseKind::Receipts => {
            let receipts = EthBlocks::block_receipts(eth_api, by_hash).await.ok()??;
            serde_json::value::to_raw_value(&receipts).ok()?
        }
    };
    let result = Arc::<RawValue>::from(raw);
    cache.insert(hash, block.number(), kind, result.clone());
    Some(SharedJson(result))
}

/// Resolves `block_id` to the hash of a recent block, or `None` if it is not one.
///
/// A hash is returned as is; whether that block is recent is checked by the cache on a hit, and
/// once it has been fetched on a miss. A hash that must be canonical is left to the original
/// handler, since a cached result does not record whether its block still is. A number is checked
/// against `best`, the best block number, before anything is looked up, so a request for an old
/// block costs no lookup here.
fn recent_block_hash<Eth>(eth_api: &Eth, block_id: BlockId, best: u64) -> Option<B256>
where
    Eth: FullEthApiServer,
{
    let number = match block_id {
        BlockId::Hash(hash) if hash.require_canonical == Some(true) => return None,
        BlockId::Hash(hash) => return Some(hash.block_hash),
        BlockId::Number(number) => number,
    };
    match number {
        BlockNumberOrTag::Pending | BlockNumberOrTag::Earliest => return None,
        BlockNumberOrTag::Number(n) if !eth_api.cache().block_responses().is_recent(n, best) => {
            return None
        }
        _ => {}
    }
    let header = eth_api.provider().sealed_header_by_number_or_tag(number).ok()??;
    Some(header.hash())
}

/// Answers with a cached result, sizing the buffer for it up front.
fn respond(
    id: jsonrpsee::types::Id<'_>,
    result: SharedJson,
    max_response_size: usize,
) -> MethodResponse {
    // The envelope around the result is the id plus a few dozen bytes.
    let capacity = result.0.get().len() + 64;
    MethodResponse::response_with_capacity(
        id,
        ResponsePayload::success(result),
        max_response_size,
        capacity,
    )
}

/// A serialized result shared between all responses that carry it.
#[derive(Debug, Clone)]
struct SharedJson(Arc<RawValue>);

impl Serialize for SharedJson {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{eth::helpers::types::EthRpcConverter, EthApi, EthApiBuilder};
    use reth_chainspec::{ChainSpec, ChainSpecProvider};
    use reth_evm_ethereum::EthEvmConfig;
    use reth_network_api::noop::NoopNetwork;
    use reth_provider::test_utils::MockEthProvider;
    use reth_rpc_eth_api::{node::RpcNodeCoreAdapter, EthApiServer};
    use reth_rpc_eth_types::EthStateCacheConfig;
    use reth_transaction_pool::test_utils::{testing_pool, TestPool};

    type TestEthApi = EthApi<
        RpcNodeCoreAdapter<MockEthProvider, TestPool, NoopNetwork, EthEvmConfig>,
        EthRpcConverter<ChainSpec>,
    >;

    fn eth_api(cache: EthStateCacheConfig) -> TestEthApi {
        let provider = MockEthProvider::default();
        EthApiBuilder::new(
            provider.clone(),
            testing_pool(),
            NoopNetwork::default(),
            EthEvmConfig::new(provider.chain_spec()),
        )
        .eth_state_cache_config(cache)
        .build()
    }

    #[tokio::test]
    async fn keeps_handler_that_is_not_async() {
        let mut module = RpcModule::new(());
        module.register_method("eth_getBlockByHash", |_, _| "sync").unwrap();

        cache_block_responses(&mut module, eth_api(Default::default()));

        assert!(matches!(module.method("eth_getBlockByHash"), Some(MethodCallback::Sync(_))));
    }

    #[tokio::test]
    async fn wraps_handlers_unless_disabled() {
        let handler = |module: &RpcModule<TestEthApi>| match module.method("eth_getBlockByHash") {
            Some(MethodCallback::Async(handler)) => handler.clone(),
            _ => panic!("no async handler"),
        };
        let is_wrapped = |cache| {
            let eth_api = eth_api(cache);
            let mut module = eth_api.clone().into_rpc();
            let original = handler(&module);
            cache_block_responses(&mut module, eth_api);
            !Arc::ptr_eq(&original, &handler(&module))
        };

        assert!(is_wrapped(EthStateCacheConfig::default()));
        assert!(!is_wrapped(EthStateCacheConfig { max_block_responses: 0, ..Default::default() }));
    }
}
