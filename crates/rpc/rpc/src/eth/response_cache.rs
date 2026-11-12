//! Serves the block and receipt methods for recent blocks from their serialized results.
//!
//! `latest` and the few blocks below it are by far the most requested blocks, and every request
//! converted and serialized the same block again. The cache keeps the serialized result per block
//! hash, so it never needs invalidating: the response for a hash cannot change, and a reorg only
//! brings new hashes.

use alloy_consensus::BlockHeader;
use alloy_eips::{BlockId, BlockNumberOrTag};
use alloy_primitives::B256;
use futures::FutureExt;
use jsonrpsee::{
    core::server::{MethodCallback, MethodResponse, RpcModule},
    types::{Params, ResponsePayload},
};
use parking_lot::Mutex;
use reth_rpc_eth_api::{
    helpers::{EthBlocks, LoadBlock},
    FullEthApiServer,
};
use reth_storage_api::{BlockNumReader, BlockReaderIdExt};
use serde::Serialize;
use serde_json::value::RawValue;
use std::{collections::VecDeque, sync::Arc};

/// How far below the best block a block's responses are cached. Older blocks are served as
/// before, so a client walking history cannot evict the blocks everyone else is asking for.
const RECENT_BLOCKS: u64 = 4;

/// Cached responses across all kinds: one of each kind for every recent block, plus the block
/// that is about to leave the window.
const CAPACITY: usize = 3 * (RECENT_BLOCKS as usize + 2);

/// Serves `eth_getBlockByNumber`, `eth_getBlockByHash` and `eth_getBlockReceipts` for recent
/// blocks from a cache of their serialized results.
///
/// Wraps the methods already registered in `module`. Whatever the cache does not serve — params it
/// cannot parse, `pending` and `earliest`, unknown and older blocks, errors — goes to the original
/// handler with the original params, so those answers are unchanged. Does nothing for a method that
/// is not registered.
pub fn cache_block_responses<Ctx, Eth>(module: &mut RpcModule<Ctx>, eth_api: Eth)
where
    Ctx: Send + Sync + 'static,
    Eth: FullEthApiServer + Clone,
{
    let cache = Arc::new(ResponseCache::default());
    wrap(module, "eth_getBlockByNumber", &eth_api, &cache, |params| {
        let (number, full) = params.parse::<(BlockNumberOrTag, bool)>().ok()?;
        Some((number.into(), Kind::block(full)))
    });
    wrap(module, "eth_getBlockByHash", &eth_api, &cache, |params| {
        let (hash, full) = params.parse::<(B256, bool)>().ok()?;
        Some((hash.into(), Kind::block(full)))
    });
    wrap(module, "eth_getBlockReceipts", &eth_api, &cache, |params| {
        let (block_id,) = params.parse::<(BlockId,)>().ok()?;
        Some((block_id, Kind::Receipts))
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
    cache: &Arc<ResponseCache>,
    request: fn(&Params<'_>) -> Option<(BlockId, Kind)>,
) where
    Ctx: Send + Sync + 'static,
    Eth: FullEthApiServer + Clone,
{
    let Some(MethodCallback::Async(original)) = module.remove_method(name) else { return };
    let (eth_api, cache) = (eth_api.clone(), cache.clone());
    let callback =
        MethodCallback::Async(Arc::new(move |id, params, conn_id, max_response_size| {
            let (eth_api, cache, original) = (eth_api.clone(), cache.clone(), original.clone());
            async move {
                let cached = match request(&params) {
                    Some((block_id, kind)) => cache.serve(&eth_api, block_id, kind).await,
                    None => None,
                };
                match cached {
                    Some(result) => respond(id, result, max_response_size),
                    None => original(id, params, conn_id, max_response_size).await,
                }
            }
            .boxed()
        }));
    module.verify_and_insert(name, callback).expect("the method was just removed");
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

/// Which response of a block is cached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// The block with full transaction objects.
    FullBlock,
    /// The block with transaction hashes only.
    HashesBlock,
    /// All receipts of the block.
    Receipts,
}

impl Kind {
    const fn block(full: bool) -> Self {
        if full {
            Self::FullBlock
        } else {
            Self::HashesBlock
        }
    }
}

/// A serialized result shared between all responses that carry it.
#[derive(Debug, Clone)]
struct SharedJson(Arc<RawValue>);

impl Serialize for SharedJson {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

/// The serialized results of the most recently requested recent blocks, oldest first.
#[derive(Debug, Default)]
struct ResponseCache {
    entries: Mutex<VecDeque<(B256, Kind, SharedJson)>>,
}

impl ResponseCache {
    /// Returns the serialized result for `block_id`, computing and caching it on a miss, or `None`
    /// if the block is not one this cache serves.
    async fn serve<Eth>(&self, eth_api: &Eth, block_id: BlockId, kind: Kind) -> Option<SharedJson>
    where
        Eth: FullEthApiServer,
    {
        let hash = recent_block_hash(eth_api, block_id)?;
        if let Some(result) = self.get(hash, kind) {
            return Some(result);
        }

        // Fetch by the resolved hash rather than the requested tag or number: `latest` may have
        // moved on since it was resolved, and the result must belong to the hash it is stored
        // under.
        let by_hash = BlockId::from(hash);
        let block = LoadBlock::recovered_block(eth_api, by_hash).await.ok()??;
        let best = eth_api.provider().best_block_number().ok()?;
        if block.number() + RECENT_BLOCKS < best {
            return None;
        }
        let raw = match kind {
            Kind::FullBlock | Kind::HashesBlock => {
                let rpc_block = EthBlocks::rpc_block(eth_api, by_hash, kind == Kind::FullBlock)
                    .await
                    .ok()??;
                serde_json::value::to_raw_value(&rpc_block).ok()?
            }
            Kind::Receipts => {
                let receipts = EthBlocks::block_receipts(eth_api, by_hash).await.ok()??;
                serde_json::value::to_raw_value(&receipts).ok()?
            }
        };
        let result = SharedJson(Arc::from(raw));
        self.insert(hash, kind, result.clone());
        Some(result)
    }

    fn get(&self, hash: B256, kind: Kind) -> Option<SharedJson> {
        self.entries
            .lock()
            .iter()
            .find(|(h, k, _)| *h == hash && *k == kind)
            .map(|(_, _, r)| r.clone())
    }

    fn insert(&self, hash: B256, kind: Kind, result: SharedJson) {
        let mut entries = self.entries.lock();
        // Requests that missed at the same time computed the same result; keep the first.
        if entries.iter().any(|(h, k, _)| *h == hash && *k == kind) {
            return;
        }
        if entries.len() == CAPACITY {
            entries.pop_front();
        }
        entries.push_back((hash, kind, result));
    }
}

/// Resolves `block_id` to the hash of a recent block, or `None` if it is not one.
///
/// A hash is returned as is; whether that block is recent is checked once it has been fetched, on a
/// miss. A hash that must be canonical is left to the original handler, since an entry does not
/// record whether its block still is. A number is checked against the best block before anything
/// is looked up, so a request for an old block costs no lookup here.
fn recent_block_hash<Eth>(eth_api: &Eth, block_id: BlockId) -> Option<B256>
where
    Eth: FullEthApiServer,
{
    let number = match block_id {
        BlockId::Hash(hash) if hash.require_canonical == Some(true) => return None,
        BlockId::Hash(hash) => return Some(hash.block_hash),
        BlockId::Number(number) => number,
    };
    let provider = eth_api.provider();
    match number {
        BlockNumberOrTag::Pending | BlockNumberOrTag::Earliest => return None,
        BlockNumberOrTag::Number(n) if n + RECENT_BLOCKS < provider.best_block_number().ok()? => {
            return None
        }
        _ => {}
    }
    let header = provider.sealed_header_by_number_or_tag(number).ok()??;
    Some(header.hash())
}
