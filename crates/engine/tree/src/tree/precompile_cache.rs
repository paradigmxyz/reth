//! Contains a precompile cache backed by `moka` with concurrent insertion deduplication.
//!
//! When prewarm starts computing a precompile result and execution arrives with the same
//! calldata, execution will wait for prewarm's result instead of recomputing.

use alloy_primitives::Bytes;
use dashmap::DashMap;
use moka::policy::EvictionPolicy;
use reth_evm::precompiles::{DynPrecompile, Precompile, PrecompileInput};
use revm::precompile::{PrecompileError, PrecompileId, PrecompileOutput, PrecompileResult};
use revm_primitives::Address;
use std::{hash::Hash, sync::Arc};

/// Default max cache size for [`PrecompileCache`]
const MAX_CACHE_SIZE: u32 = 10_000;

/// Stores caches for each precompile.
#[derive(Debug, Clone, Default)]
pub struct PrecompileCacheMap<S>(Arc<DashMap<Address, PrecompileCache<S>>>)
where
    S: Eq + Hash + std::fmt::Debug + Send + Sync + Clone + 'static;

impl<S> PrecompileCacheMap<S>
where
    S: Eq + Hash + std::fmt::Debug + Send + Sync + Clone + 'static,
{
    pub(crate) fn cache_for_address(&self, address: Address) -> PrecompileCache<S> {
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
pub struct PrecompileCache<S>(
    moka::sync::Cache<Bytes, CacheEntry<S>, alloy_primitives::map::DefaultHashBuilder>,
)
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
                .build_with_hasher(Default::default()),
        )
    }
}

impl<S> PrecompileCache<S>
where
    S: Eq + Hash + std::fmt::Debug + Send + Sync + Clone + 'static,
{
    /// Fast path: check if result is already cached.
    fn get(&self, input: &[u8]) -> Option<CacheEntry<S>> {
        self.0.get(input)
    }

    /// Gets the cached result or computes it using the provided closure.
    ///
    /// If another thread is already computing the result for this input, this will wait
    /// for that computation to complete instead of running the closure again.
    fn get_or_try_insert_with<E>(
        &self,
        input: Bytes,
        spec: S,
        init: impl FnOnce() -> Result<PrecompileOutput, E>,
    ) -> Result<CacheEntry<S>, Arc<E>>
    where
        E: Send + Sync + 'static,
    {
        self.0.try_get_with(input, || init().map(|output| CacheEntry { output, spec }))
    }

    fn entry_count(&self) -> usize {
        self.0.entry_count() as usize
    }
}

/// Cache entry, precompile successful output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheEntry<S> {
    output: PrecompileOutput,
    spec: S,
}

impl<S> CacheEntry<S> {
    const fn gas_used(&self) -> u64 {
        self.output.gas_used
    }

    fn to_precompile_result(&self) -> PrecompileResult {
        Ok(self.output.clone())
    }
}

/// A cache for precompile inputs / outputs.
#[derive(Debug)]
pub(crate) struct CachedPrecompile<S>
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
    pub(crate) const fn new(
        precompile: DynPrecompile,
        cache: PrecompileCache<S>,
        spec_id: S,
        metrics: Option<CachedPrecompileMetrics>,
    ) -> Self {
        Self { precompile, cache, spec_id, metrics }
    }

    pub(crate) fn wrap(
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
        // Fast path: check cache without allocation
        if let Some(entry) = self.cache.get(input.data).filter(|e| input.gas >= e.gas_used()) {
            self.increment_by_one_precompile_cache_hits();
            return entry.to_precompile_result();
        }

        // Slow path: compute result, waiting for in-flight computation if another thread started
        let calldata = Bytes::copy_from_slice(input.data);
        let gas_limit = input.gas;

        let result = self
            .cache
            .get_or_try_insert_with(calldata, self.spec_id.clone(), || self.precompile.call(input));

        match result {
            Ok(entry) => {
                self.set_precompile_cache_size_metric(self.cache.entry_count() as f64);
                if gas_limit >= entry.gas_used() {
                    self.increment_by_one_precompile_cache_hits();
                    entry.to_precompile_result()
                } else {
                    Err(PrecompileError::OutOfGas)
                }
            }
            Err(err) => {
                self.increment_by_one_precompile_errors();
                Err(Arc::unwrap_or_clone(err))
            }
        }
    }
}

/// Metrics for the cached precompile.
#[derive(reth_metrics::Metrics, Clone)]
#[metrics(scope = "sync.caching")]
pub(crate) struct CachedPrecompileMetrics {
    /// Precompile cache hits
    precompile_cache_hits: metrics::Counter,

    /// Precompile cache size. Uses the LRU cache length as the size metric.
    precompile_cache_size: metrics::Gauge,

    /// Precompile execution errors.
    precompile_errors: metrics::Counter,
}

impl CachedPrecompileMetrics {
    /// Creates a new instance of [`CachedPrecompileMetrics`] with the given address.
    ///
    /// Adds address as an `address` label padded with zeros to at least two hex symbols, prefixed
    /// by `0x`.
    pub(crate) fn new_with_address(address: Address) -> Self {
        Self::new_with_labels(&[("address", format!("0x{address:02x}"))])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_evm::{EthEvmFactory, Evm, EvmEnv, EvmFactory};
    use reth_revm::db::EmptyDB;
    use revm::{context::TxEnv, precompile::PrecompileOutput};
    use revm_primitives::hardfork::SpecId;

    #[test]
    fn test_precompile_cache_basic() {
        let expected_output = PrecompileOutput {
            gas_used: 50,
            gas_refunded: 0,
            bytes: alloy_primitives::Bytes::copy_from_slice(b"cached_result"),
            reverted: false,
        };

        let cache: PrecompileCache<SpecId> = PrecompileCache::default();

        let result = cache
            .get_or_try_insert_with(Bytes::from_static(b"test_input"), SpecId::PRAGUE, || {
                Ok::<_, PrecompileError>(expected_output.clone())
            })
            .unwrap();

        assert_eq!(result.output, expected_output);
        assert_eq!(result.spec, SpecId::PRAGUE);

        // Second call should return cached result without calling init
        let result2 = cache
            .get_or_try_insert_with(
                Bytes::from_static(b"test_input"),
                SpecId::PRAGUE,
                || -> Result<PrecompileOutput, PrecompileError> {
                    panic!("should not be called - result should be cached")
                },
            )
            .unwrap();

        assert_eq!(result2.output, expected_output);
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
                    gas_used: 5000,
                    gas_refunded: 0,
                    bytes: alloy_primitives::Bytes::copy_from_slice(b"output_from_precompile_1"),
                    reverted: false,
                })
            }
        })
            .into();

        // create the second precompile with a different output
        let precompile2: DynPrecompile = (PrecompileId::custom("custom"), {
            move |input: PrecompileInput<'_>| -> PrecompileResult {
                assert_eq!(input.data, input_data);

                Ok(PrecompileOutput {
                    gas_used: 7000,
                    gas_refunded: 0,
                    bytes: alloy_primitives::Bytes::copy_from_slice(b"output_from_precompile_2"),
                    reverted: false,
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
    fn test_concurrent_deduplication() {
        use std::{
            sync::atomic::{AtomicUsize, Ordering},
            thread,
            time::Duration,
        };

        let cache: PrecompileCache<SpecId> = PrecompileCache::default();
        let cache = Arc::new(cache);
        let call_count = Arc::new(AtomicUsize::new(0));

        let expected_output = PrecompileOutput {
            gas_used: 100,
            gas_refunded: 0,
            bytes: alloy_primitives::Bytes::copy_from_slice(b"result"),
            reverted: false,
        };

        let mut handles = vec![];

        for _ in 0..10 {
            let cache = Arc::clone(&cache);
            let call_count = Arc::clone(&call_count);
            let expected = expected_output.clone();

            handles.push(thread::spawn(move || {
                cache
                    .get_or_try_insert_with(
                        Bytes::from_static(b"same_input"),
                        SpecId::PRAGUE,
                        || {
                            call_count.fetch_add(1, Ordering::SeqCst);
                            // Small delay to increase chance of concurrent access
                            thread::sleep(Duration::from_millis(10));
                            Ok::<_, PrecompileError>(expected.clone())
                        },
                    )
                    .unwrap()
            }));
        }

        for handle in handles {
            let result = handle.join().unwrap();
            assert_eq!(result.output, expected_output);
        }

        // With concurrent deduplication, init should only be called once
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }
}
