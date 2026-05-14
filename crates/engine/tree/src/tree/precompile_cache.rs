//! Contains a precompile cache backed by `schnellru::LruMap` (LRU by length).

use alloy_primitives::{
    map::{DefaultHashBuilder, FbBuildHasher},
    Bytes,
};
use moka::policy::EvictionPolicy;
use reth_evm::precompiles::{DynPrecompile, Precompile, PrecompileInput};
use reth_primitives_traits::dashmap::DashMap;
use revm::precompile::{PrecompileId, PrecompileOutput, PrecompileResult};
use revm_primitives::Address;
use std::{hash::Hash, sync::Arc};
use tracing::error;

/// Default max cache size for [`PrecompileCache`]
const MAX_CACHE_SIZE: u32 = 1024 * 1024;

/// Stores caches for each precompile.
#[derive(Debug, Clone, Default)]
pub struct PrecompileCacheMap<S>(Arc<DashMap<Address, PrecompileCache<S>, FbBuildHasher<20>>>)
where
    S: Eq + Hash + std::fmt::Debug + Send + Sync + Clone + 'static;

impl<S> PrecompileCacheMap<S>
where
    S: Eq + Hash + std::fmt::Debug + Send + Sync + Clone + 'static,
{
    /// Get the precompile cache for the given address.
    pub fn cache_for_address(&self, address: Address) -> PrecompileCache<S> {
        // Try just using `.get` first to avoid acquiring a write lock.
        if let Some(cache) = self.0.get(&address) {
            return cache.clone();
        }
        // Otherwise, fallback to `.entry` and initialize the cache.
        //
        // This should be very rare as caches for all precompiles will be initialized as soon as
        // first EVM is created.
        self.0.entry(address).or_default().clone()
    }
}

/// Cache for precompiles, for each input stores the result.
#[derive(Debug, Clone)]
pub struct PrecompileCache<S>(moka::sync::Cache<Bytes, CacheEntry<S>, DefaultHashBuilder>)
where
    S: Eq + Hash + std::fmt::Debug + Send + Sync + Clone + 'static;

impl<S> Default for PrecompileCache<S>
where
    S: Eq + Hash + std::fmt::Debug + Send + Sync + Clone + 'static,
{
    fn default() -> Self {
        Self(
            moka::sync::CacheBuilder::new(MAX_CACHE_SIZE as u64)
                .initial_capacity(MAX_CACHE_SIZE as usize)
                .eviction_policy(EvictionPolicy::lru())
                .weigher(|key: &Bytes, value: &CacheEntry<S>| {
                    (key.len() + value.output.bytes.len()) as u32
                })
                .build_with_hasher(Default::default()),
        )
    }
}

impl<S> PrecompileCache<S>
where
    S: Eq + Hash + std::fmt::Debug + Send + Sync + Clone + 'static,
{
    fn get(&self, input: &[u8], spec: S) -> Option<CacheEntry<S>> {
        self.0.get(input).filter(|e| e.spec == spec)
    }

    /// Inserts the given key and value into the cache, returning the new cache size.
    fn insert(&self, input: Bytes, value: CacheEntry<S>) -> usize {
        self.0.insert(input, value);
        self.0.entry_count() as usize
    }
}

/// Cache entry for a successful precompile output.
///
/// We intentionally do not cache non-successful statuses or errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheEntry<S> {
    output: PrecompileOutput,
    spec: S,
}

impl<S> CacheEntry<S> {
    const fn gas_used(&self) -> u64 {
        self.output.gas_used
    }

    /// Converts the cache entry to a precompile result. Accepts state gas reservoir as input.
    ///
    /// All cached precompiles are not expected to access/created state and thus reservoir is always
    /// kept as is.
    fn to_precompile_result(&self, reservoir: u64) -> PrecompileResult {
        let mut output = self.output.clone();
        output.reservoir = reservoir;
        Ok(output)
    }
}

/// A cache for precompile inputs / outputs.
#[derive(Debug)]
pub struct CachedPrecompile<S>
where
    S: Eq + Hash + std::fmt::Debug + Send + Sync + Clone + 'static,
{
    /// Cache for precompile results and gas bounds.
    cache: PrecompileCache<S>,
    /// The precompile.
    precompile: DynPrecompile,
    /// Cache metrics.
    metrics: Option<CachedPrecompileMetrics>,
    /// Spec id associated to the EVM from which this cached precompile was created.
    spec_id: S,
}

impl<S> CachedPrecompile<S>
where
    S: Eq + Hash + std::fmt::Debug + Send + Sync + Clone + 'static,
{
    /// `CachedPrecompile` constructor.
    pub const fn new(
        precompile: DynPrecompile,
        cache: PrecompileCache<S>,
        spec_id: S,
        metrics: Option<CachedPrecompileMetrics>,
    ) -> Self {
        Self { precompile, cache, spec_id, metrics }
    }

    /// Wrap the given precompile in a cached precompile.
    pub fn wrap(
        precompile: DynPrecompile,
        cache: PrecompileCache<S>,
        spec_id: S,
        metrics: Option<CachedPrecompileMetrics>,
    ) -> DynPrecompile {
        let precompile_id = precompile.precompile_id().clone();
        let wrapped = Self::new(precompile, cache, spec_id, metrics);
        (precompile_id, move |input: PrecompileInput<'_>| -> PrecompileResult {
            wrapped.call(input)
        })
            .into()
    }

    fn increment_by_one_precompile_cache_hits(&self) {
        if let Some(metrics) = &self.metrics {
            metrics.precompile_cache_hits.increment(1);
        }
    }

    fn increment_by_one_precompile_cache_misses(&self) {
        if let Some(metrics) = &self.metrics {
            metrics.precompile_cache_misses.increment(1);
        }
    }

    fn set_precompile_cache_size_metric(&self, to: f64) {
        if let Some(metrics) = &self.metrics {
            metrics.precompile_cache_size.set(to);
        }
    }

    fn increment_by_one_precompile_errors(&self) {
        if let Some(metrics) = &self.metrics {
            metrics.precompile_errors.increment(1);
        }
    }
}

impl<S> Precompile for CachedPrecompile<S>
where
    S: Eq + Hash + std::fmt::Debug + Send + Sync + Clone + 'static,
{
    fn precompile_id(&self) -> &PrecompileId {
        self.precompile.precompile_id()
    }

    fn call(&self, input: PrecompileInput<'_>) -> PrecompileResult {
        if let Some(entry) = &self.cache.get(input.data, self.spec_id.clone()) &&
            input.gas >= entry.gas_used()
        {
            self.increment_by_one_precompile_cache_hits();
            return entry.to_precompile_result(input.reservoir);
        }

        let calldata = input.data;
        let reservoir = input.reservoir;
        let result = self.precompile.call(input);

        match &result {
            // Only successful outputs are cacheable. Non-success statuses and errors must execute
            // again instead of poisoning the cache for subsequent calls.
            Ok(output) if output.is_success() => {
                // Sanity-check precompile output to ensure that it does not affect state gas in any
                // way.
                //
                // This does not fully protect us from caching stateful precompiles but might make
                // it obvious when the node is misconfigured.
                if output.reservoir != reservoir {
                    error!(target: "engine::tree", precompile_id = self.precompile.precompile_id().name(), "cacheable precompile decremented reservoir, skipping cache insertion");
                } else if output.state_gas_used != 0 {
                    error!(target: "engine::tree", precompile_id = self.precompile.precompile_id().name(), "cacheable precompile used state gas, skipping cache insertion");
                } else {
                    let size = self.cache.insert(
                        Bytes::copy_from_slice(calldata),
                        CacheEntry { output: output.clone(), spec: self.spec_id.clone() },
                    );
                    self.set_precompile_cache_size_metric(size as f64);
                    self.increment_by_one_precompile_cache_misses();
                }
            }
            _ => {
                self.increment_by_one_precompile_errors();
            }
        }
        result
    }
}

/// Metrics for the cached precompile.
#[derive(reth_metrics::Metrics, Clone)]
#[metrics(scope = "sync.caching")]
pub struct CachedPrecompileMetrics {
    /// Precompile cache hits
    pub precompile_cache_hits: metrics::Counter,

    /// Precompile cache misses
    pub precompile_cache_misses: metrics::Counter,

    /// Precompile cache size. Uses the LRU cache length as the size metric.
    pub precompile_cache_size: metrics::Gauge,

    /// Precompile execution errors.
    pub precompile_errors: metrics::Counter,
}

impl CachedPrecompileMetrics {
    /// Creates a new instance of [`CachedPrecompileMetrics`] with the given address.
    ///
    /// Adds address as an `address` label padded with zeros to at least two hex symbols, prefixed
    /// by `0x`.
    pub fn new_with_address(address: Address) -> Self {
        Self::new_with_labels(&[("address", format!("0x{address:02x}"))])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_evm::{EthEvmFactory, Evm, EvmEnv, EvmFactory};
    use reth_revm::db::EmptyDB;
    use revm::{
        context::TxEnv,
        precompile::{PrecompileOutput, PrecompileStatus},
    };
    use revm_primitives::hardfork::SpecId;

    #[test]
    fn test_precompile_cache_basic() {
        let dyn_precompile: DynPrecompile = (|_input: PrecompileInput<'_>| -> PrecompileResult {
            Ok(PrecompileOutput {
                status: PrecompileStatus::Success,
                gas_used: 0,
                state_gas_used: 0,
                reservoir: 0,
                gas_refunded: 0,
                bytes: Bytes::default(),
            })
        })
        .into();

        let cache =
            CachedPrecompile::new(dyn_precompile, PrecompileCache::default(), SpecId::PRAGUE, None);

        let output = PrecompileOutput {
            status: PrecompileStatus::Success,
            gas_used: 50,
            state_gas_used: 0,
            reservoir: 0,
            gas_refunded: 0,
            bytes: alloy_primitives::Bytes::copy_from_slice(b"cached_result"),
        };

        let input = b"test_input";
        let expected = CacheEntry { output, spec: SpecId::PRAGUE };
        cache.cache.insert(input.into(), expected.clone());

        let actual = cache.cache.get(input, SpecId::PRAGUE).unwrap();

        assert_eq!(actual, expected);
    }

    #[test]
    fn test_precompile_cache_map_separate_addresses() {
        let mut evm = EthEvmFactory::default().create_evm(EmptyDB::default(), EvmEnv::default());
        let input_data = b"same_input";
        let gas_limit = 100_000;

        let address1 = Address::repeat_byte(1);
        let address2 = Address::repeat_byte(2);

        let cache_map = PrecompileCacheMap::default();

        // create the first precompile with a specific output
        let precompile1: DynPrecompile = (PrecompileId::custom("custom"), {
            move |input: PrecompileInput<'_>| -> PrecompileResult {
                assert_eq!(input.data, input_data);

                Ok(PrecompileOutput {
                    status: PrecompileStatus::Success,
                    gas_used: 5000,
                    state_gas_used: 0,
                    reservoir: 0,
                    gas_refunded: 0,
                    bytes: alloy_primitives::Bytes::copy_from_slice(b"output_from_precompile_1"),
                })
            }
        })
            .into();

        // create the second precompile with a different output
        let precompile2: DynPrecompile = (PrecompileId::custom("custom"), {
            move |input: PrecompileInput<'_>| -> PrecompileResult {
                assert_eq!(input.data, input_data);

                Ok(PrecompileOutput {
                    status: PrecompileStatus::Success,
                    gas_used: 7000,
                    state_gas_used: 0,
                    reservoir: 0,
                    gas_refunded: 0,
                    bytes: alloy_primitives::Bytes::copy_from_slice(b"output_from_precompile_2"),
                })
            }
        })
            .into();

        let wrapped_precompile1 = CachedPrecompile::wrap(
            precompile1,
            cache_map.cache_for_address(address1),
            SpecId::PRAGUE,
            None,
        );
        let wrapped_precompile2 = CachedPrecompile::wrap(
            precompile2,
            cache_map.cache_for_address(address2),
            SpecId::PRAGUE,
            None,
        );

        let precompile1_address = Address::with_last_byte(1);
        let precompile2_address = Address::with_last_byte(2);

        evm.precompiles_mut().apply_precompile(&precompile1_address, |_| Some(wrapped_precompile1));
        evm.precompiles_mut().apply_precompile(&precompile2_address, |_| Some(wrapped_precompile2));

        // first invocation of precompile1 (cache miss)
        let result1 = evm
            .transact_raw(TxEnv {
                caller: Address::ZERO,
                gas_limit,
                data: input_data.into(),
                kind: precompile1_address.into(),
                ..Default::default()
            })
            .unwrap()
            .result
            .into_output()
            .unwrap();
        assert_eq!(result1.as_ref(), b"output_from_precompile_1");

        // first invocation of precompile2 with the same input (should be a cache miss)
        // if cache was incorrectly shared, we'd get precompile1's result
        let result2 = evm
            .transact_raw(TxEnv {
                caller: Address::ZERO,
                gas_limit,
                data: input_data.into(),
                kind: precompile2_address.into(),
                ..Default::default()
            })
            .unwrap()
            .result
            .into_output()
            .unwrap();
        assert_eq!(result2.as_ref(), b"output_from_precompile_2");

        // second invocation of precompile1 (should be a cache hit)
        let result3 = evm
            .transact_raw(TxEnv {
                caller: Address::ZERO,
                gas_limit,
                data: input_data.into(),
                kind: precompile1_address.into(),
                ..Default::default()
            })
            .unwrap()
            .result
            .into_output()
            .unwrap();
        assert_eq!(result3.as_ref(), b"output_from_precompile_1");
    }
}
