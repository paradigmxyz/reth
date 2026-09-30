//! Contains a precompile cache backed by `schnellru::LruMap` (LRU by length).

use alloy_primitives::{
    map::{DefaultHashBuilder, FbBuildHasher},
    Address, Bytes,
};
use moka::policy::EvictionPolicy;
use reth_evm::{
    precompiles::{DynPrecompile, Precompile, PrecompileInput, PrecompilesMap},
    Evm,
};
use reth_payload_builder::SharedPrecompileCache;
use reth_primitives_traits::dashmap::DashMap;
use revm::precompile::{PrecompileId, PrecompileOutput, PrecompileResult};
use std::{hash::Hash, sync::Arc};
use tracing::error;

/// Default max cache size for [`PrecompileCache`]
const MAX_CACHE_SIZE: u32 = 1024 * 1024;

/// Maximum calldata size to cache for a precompile.
const MAX_PRECOMPILE_CACHE_INPUT_SIZE: usize = 2 * 1024;

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

    /// Type-erases this map so it can be loaned to the payload builder.
    pub fn into_shared(self) -> SharedPrecompileCache {
        SharedPrecompileCache::new(self)
    }

    /// Returns the map if `shared` was created from a [`PrecompileCacheMap`] with spec type `S`.
    pub fn from_shared(shared: &SharedPrecompileCache) -> Option<Self> {
        shared.downcast()
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
        let cacheable_input = input.data.len() <= MAX_PRECOMPILE_CACHE_INPUT_SIZE;
        if cacheable_input &&
            let Some(entry) = &self.cache.get(input.data, self.spec_id.clone()) &&
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
            Ok(output) if cacheable_input && output.is_success() => {
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
            // Oversized successful inputs execute normally but are not cacheable.
            Ok(output) if output.is_success() => {}
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

/// Wraps the cacheable precompiles of `evm` with the shared precompile cache.
///
/// The cache map is recovered for the EVM's spec type and entries are tied to the EVM's current
/// spec. Wrapped precompiles do not record metrics, so lookups from other EVMs do not skew the
/// engine's precompile cache metrics.
///
/// Returns `false` and leaves the precompiles unchanged if `shared` was created for a different
/// spec type.
pub fn wrap_with_shared_precompile_cache<E>(evm: &mut E, shared: &SharedPrecompileCache) -> bool
where
    E: Evm<Precompiles = PrecompilesMap>,
{
    let Some(cache_map) = PrecompileCacheMap::<E::Spec>::from_shared(shared) else { return false };
    let spec_id = evm.cfg_env().spec;
    evm.precompiles_mut().map_cacheable_precompiles(|address, precompile| {
        CachedPrecompile::wrap(precompile, cache_map.cache_for_address(*address), spec_id, None)
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use metrics_util::debugging::{DebugValue, DebuggingRecorder};
    use reth_evm::{EthEvmFactory, EvmEnv, EvmFactory};
    use reth_revm::db::EmptyDB;
    use revm::{
        context::TxEnv,
        precompile::{PrecompileOutput, PrecompileStatus},
        primitives::hardfork::SpecId,
    };

    #[test]
    fn test_precompile_cache_basic() {
        let dyn_precompile: DynPrecompile = (|_input: PrecompileInput<'_>| -> PrecompileResult {
            Ok(PrecompileOutput {
                status: PrecompileStatus::Success,
                gas_used: 0,
                state_gas_used: 0,
                state_gas_spilled: 0,
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
            state_gas_spilled: 0,
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
                    state_gas_spilled: 0,
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
                    state_gas_spilled: 0,
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

    #[test]
    fn test_oversized_successful_input_is_not_an_error() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let cache = PrecompileCache::default();
        let input_data = Bytes::from(vec![0; MAX_PRECOMPILE_CACHE_INPUT_SIZE + 1]);
        let address = Address::with_last_byte(1);

        metrics::with_local_recorder(&recorder, || {
            let precompile: DynPrecompile = (|_input: PrecompileInput<'_>| {
                Ok(PrecompileOutput {
                    status: PrecompileStatus::Success,
                    gas_used: 0,
                    state_gas_used: 0,
                    state_gas_spilled: 0,
                    reservoir: 0,
                    gas_refunded: 0,
                    bytes: Bytes::default(),
                })
            })
            .into();
            let wrapped = CachedPrecompile::wrap(
                precompile,
                cache.clone(),
                SpecId::PRAGUE,
                Some(CachedPrecompileMetrics::new_with_address(address)),
            );
            let mut evm =
                EthEvmFactory::default().create_evm(EmptyDB::default(), EvmEnv::default());
            evm.precompiles_mut().apply_precompile(&address, |_| Some(wrapped));

            evm.transact_raw(TxEnv {
                caller: Address::ZERO,
                gas_limit: 100_000,
                data: input_data.clone(),
                kind: address.into(),
                ..Default::default()
            })
            .unwrap();
        });

        assert!(cache.get(&input_data, SpecId::PRAGUE).is_none());
        let error_count = snapshotter.snapshot().into_vec().into_iter().find_map(
            |(key, _unit, _description, value)| {
                (key.key().name() == "sync.caching.precompile_errors").then_some(value)
            },
        );
        assert_eq!(error_count, Some(DebugValue::Counter(0)));
    }

    #[test]
    fn test_shared_precompile_cache_downcast() {
        let cache_map = PrecompileCacheMap::<SpecId>::default();
        let shared = cache_map.clone().into_shared();

        let recovered = PrecompileCacheMap::<SpecId>::from_shared(&shared).unwrap();
        assert!(Arc::ptr_eq(&cache_map.0, &recovered.0));

        // a map for a different spec type must not be recovered
        assert!(PrecompileCacheMap::<u8>::from_shared(&shared).is_none());
    }

    #[test]
    fn test_wrap_with_shared_precompile_cache() {
        let sha256 = Address::with_last_byte(2);
        let input_data = b"shared_input";
        let cache_map = PrecompileCacheMap::<SpecId>::default();
        let mut evm = EthEvmFactory::default().create_evm(EmptyDB::default(), EvmEnv::default());
        let spec_id = evm.cfg_env().spec;

        // a cache for a different spec type is rejected
        assert!(!wrap_with_shared_precompile_cache(
            &mut evm,
            &PrecompileCacheMap::<u8>::default().into_shared()
        ));

        assert!(wrap_with_shared_precompile_cache(&mut evm, &cache_map.clone().into_shared()));

        let output = evm
            .transact_raw(TxEnv {
                caller: Address::ZERO,
                gas_limit: 100_000,
                data: input_data.into(),
                kind: sha256.into(),
                ..Default::default()
            })
            .unwrap()
            .result
            .into_output()
            .unwrap();

        // the result is stored in the shared map under the EVM's spec
        let entry = cache_map.cache_for_address(sha256).get(input_data, spec_id).unwrap();
        assert_eq!(entry.output.bytes, output);
    }
}
