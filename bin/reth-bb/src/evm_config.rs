//! EVM configuration for merged big-block payloads.

pub(crate) use reth_engine_primitives::BigBlockData;

use alloy_consensus::Header;
use alloy_eips::Decodable2718;
use alloy_primitives::{Bytes, U256};
use alloy_rpc_types::engine::ExecutionData;
use core::convert::Infallible;
use reth_chainspec::{ChainSpec, EthChainSpec, EthereumHardforks};
use reth_engine_primitives::ExecutionPayload;
use reth_ethereum_forks::Hardforks;
use reth_ethereum_primitives::{Block, EthPrimitives, TransactionSigned};
use reth_evm::{
    precompile_cache::PrecompileCacheMap, BlockAssembler, BlockAssemblerInput, BlockExecutionError,
    ConfigureEngineEvm, ConfigureEvm, EvmEnvFor, EvmFor, ExecutableTxIterator, ExecutionCtxFor,
    NextBlockEnvAttributes, SpecFor,
};
use reth_evm_ethereum::{
    EthBigBlockExecutorFactory, EthBigBlockPlan, EthBigBlockSegment, EthEvmConfig,
    ExecutableRecoveredTx,
};
use reth_primitives_traits::{BlockTy, HeaderTy, SealedBlock, SealedHeader, SignedTransaction};
use reth_storage_errors::any::AnyError;

/// Block assembler marker for replay-only big-block execution.
#[derive(Debug, Clone, Copy, Default)]
pub struct BbBlockAssembler;

impl<C> BlockAssembler<EthBigBlockExecutorFactory<C>> for BbBlockAssembler
where
    C: EthChainSpec<Header = Header> + EthereumHardforks + 'static,
{
    type Block = Block;

    fn assemble_block(
        &self,
        _input: BlockAssemblerInput<'_, '_, EthBigBlockExecutorFactory<C>, Header>,
    ) -> Result<Self::Block, BlockExecutionError> {
        unreachable!("big-block execution is replay-only")
    }
}

/// EVM configuration for big-block execution.
#[derive(Debug, Clone)]
pub struct BbEvmConfig<C = ChainSpec> {
    inner: EthEvmConfig<C>,
    executor_factory: EthBigBlockExecutorFactory<C>,
    block_assembler: BbBlockAssembler,
}

impl<C> BbEvmConfig<C> {
    /// Creates a big-block EVM configuration from the standard Ethereum configuration.
    pub fn new(inner: EthEvmConfig<C>) -> Self
    where
        EthEvmConfig<C>: Clone,
        EthBigBlockExecutorFactory<C>: Clone,
    {
        let executor_factory = EthBigBlockExecutorFactory::new(inner.executor_factory.clone());
        Self { inner, executor_factory, block_assembler: BbBlockAssembler }
    }
}

impl<C> ConfigureEvm for BbEvmConfig<C>
where
    C: EthChainSpec<Header = Header> + EthereumHardforks + Hardforks + 'static,
{
    type Primitives = EthPrimitives;
    type Error = Infallible;
    type NextBlockEnvCtx = NextBlockEnvAttributes;
    type BlockExecutorFactory = EthBigBlockExecutorFactory<C>;
    type BlockAssembler = BbBlockAssembler;

    fn evm_with_precompile_cache<'a, DB>(
        &self,
        db: DB,
        evm_env: EvmEnvFor<Self>,
        cache_map: PrecompileCacheMap<SpecFor<Self>>,
        metrics_enabled: bool,
    ) -> EvmFor<'a, Self>
    where
        DB: evm2::evm::Database + 'a,
    {
        self.inner.evm_with_precompile_cache(db, evm_env, cache_map, metrics_enabled)
    }

    fn block_executor_factory(&self) -> &Self::BlockExecutorFactory {
        &self.executor_factory
    }

    fn with_jit_support_enabled(self, enabled: bool) -> Self {
        Self::new(self.inner.with_jit_support_enabled(enabled))
    }

    fn jit_backend(&self) -> Option<&dyn reth_evm::JitBackend> {
        self.inner.jit_backend()
    }

    fn block_assembler(&self) -> &Self::BlockAssembler {
        &self.block_assembler
    }

    fn evm_env(&self, header: &HeaderTy<Self::Primitives>) -> Result<EvmEnvFor<Self>, Self::Error> {
        self.inner.evm_env(header)
    }

    fn next_evm_env(
        &self,
        parent: &HeaderTy<Self::Primitives>,
        attributes: &Self::NextBlockEnvCtx,
    ) -> Result<EvmEnvFor<Self>, Self::Error> {
        self.inner.next_evm_env(parent, attributes)
    }

    fn context_for_block<'a>(
        &self,
        _block: &'a SealedBlock<BlockTy<Self::Primitives>>,
    ) -> Result<ExecutionCtxFor<'a, Self>, Self::Error>
    where
        Self: 'a,
    {
        unreachable!("big blocks EVM should only be used from within the engine pipeline")
    }

    fn context_for_next_block(
        &self,
        _parent: &SealedHeader<HeaderTy<Self::Primitives>>,
        _attributes: Self::NextBlockEnvCtx,
    ) -> Result<ExecutionCtxFor<'_, Self>, Self::Error> {
        unreachable!("big blocks EVM should only be used from within the engine pipeline")
    }
}

impl<C> ConfigureEngineEvm<BigBlockData<ExecutionData>> for BbEvmConfig<C>
where
    C: EthChainSpec<Header = Header> + EthereumHardforks + Hardforks + 'static,
{
    fn evm_env_for_payload(
        &self,
        payload: &BigBlockData<ExecutionData>,
    ) -> Result<EvmEnvFor<Self>, Self::Error> {
        let first = payload.env_switches.first().expect("big-block payload has no segments");
        let mut env = self.inner.evm_env_for_payload(first)?;
        // Prewarming uses this environment for transactions from every segment, whose fee caps
        // can be below the first segment's base fee.
        env.version.features.remove(evm2::EvmFeatures::BASE_FEE_CHECK);
        env.block.gas_limit = U256::from(payload.gas_limit());
        Ok(env)
    }

    fn context_for_payload<'a>(
        &self,
        payload: &'a BigBlockData<ExecutionData>,
    ) -> Result<ExecutionCtxFor<'a, Self>, Self::Error> {
        assert!(!payload.env_switches.is_empty(), "big-block payload has no segments");

        let mut start_tx = 0;
        let mut segments = Vec::with_capacity(payload.env_switches.len());
        for data in &payload.env_switches {
            let evm_env = self.inner.evm_env_for_payload(data)?;
            let ctx = self.inner.context_for_payload(data)?;
            segments.push(EthBigBlockSegment { start_tx, evm_env, ctx });
            start_tx += data.payload.transactions().len();
        }

        Ok(EthBigBlockPlan::new(segments, payload.prior_block_hashes.clone(), start_tx))
    }

    fn tx_iterator_for_payload(
        &self,
        payload: &BigBlockData<ExecutionData>,
    ) -> Result<impl ExecutableTxIterator<Self>, Self::Error> {
        let transactions = payload
            .env_switches
            .iter()
            .flat_map(|data| data.payload.transactions().iter().cloned())
            .collect::<Vec<_>>();
        let convert = |tx: Bytes| {
            let tx = TransactionSigned::decode_2718_exact(tx.as_ref()).map_err(AnyError::new)?;
            let signer = tx.try_recover().map_err(AnyError::new)?;
            Ok::<_, AnyError>(ExecutableRecoveredTx::new(tx.with_signer(signer)))
        };

        Ok((transactions, convert))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::TxLegacy;
    use alloy_primitives::{Address, Signature, TxKind};
    use alloy_rpc_types::engine::{ExecutionPayloadSidecar, ExecutionPayloadV1};
    use evm2::evm::{EmptyDB, SystemTx};
    use metrics_util::debugging::{DebugValue, DebuggingRecorder};
    use reth_evm::EvmEnv;
    use reth_evm_ethereum::EthEvmEnv;

    #[test]
    fn prewarm_accepts_later_segment_with_lower_base_fee() {
        let segment = |base_fee| ExecutionData {
            payload: ExecutionPayloadV1::from_block_unchecked(
                Default::default(),
                &Block {
                    header: Header {
                        number: 16_000_000,
                        timestamp: 1_670_000_000,
                        gas_limit: 30_000_000,
                        base_fee_per_gas: Some(base_fee),
                        ..Default::default()
                    },
                    body: Default::default(),
                },
            )
            .into(),
            sidecar: ExecutionPayloadSidecar::none(),
        };
        let payload = BigBlockData {
            env_switches: vec![segment(100u64), segment(50u64)],
            prior_block_hashes: Vec::new(),
            block_number: 16_000_000,
            merged_block_access_list: None,
        };
        let config = BbEvmConfig::new(EthEvmConfig::mainnet());
        let env = config
            .evm_env_for_payload(&payload)
            .unwrap()
            .with_nonce_check_disabled()
            .with_balance_check_disabled();
        let mut evm = config.evm_with_env(EmptyDB::default(), env);
        let tx = TransactionSigned::new_unhashed(
            TxLegacy {
                gas_price: 50,
                gas_limit: 21_000,
                to: TxKind::Call(Address::with_last_byte(100)),
                ..Default::default()
            }
            .into(),
            Signature::new(U256::from(1), U256::from(1), false),
        )
        .with_signer(Address::with_last_byte(101))
        .convert();
        assert!(evm.transact(&tx).unwrap().discard().status);
        assert_eq!(evm.block().gas_limit, U256::from(60_000_000));
        assert!(config
            .context_for_payload(&payload)
            .unwrap()
            .segments
            .iter()
            .all(|segment| segment.evm_env.version.feature(evm2::EvmFeatures::BASE_FEE_CHECK)));
    }

    #[test]
    fn precompile_cache_is_opt_in_per_evm() {
        let recorder = DebuggingRecorder::new();
        metrics::with_local_recorder(&recorder, || {
            let metrics_snapshot = || {
                recorder
                    .snapshotter()
                    .snapshot()
                    .into_vec()
                    .into_iter()
                    .filter(|(key, ..)| key.key().name().starts_with("sync.caching.precompile"))
                    .collect::<Vec<_>>()
            };
            let config = BbEvmConfig::new(EthEvmConfig::mainnet());
            let cache_map = PrecompileCacheMap::default();
            let identity = Address::with_last_byte(4);
            let sha256 = Address::with_last_byte(2);
            let ecadd = Address::with_last_byte(6);
            let call = |cache_metrics: Option<bool>, address, input: Bytes| {
                let mut evm = match cache_metrics {
                    Some(metrics_enabled) => config.evm_with_precompile_cache(
                        EmptyDB::default(),
                        EthEvmEnv::default(),
                        cache_map.clone(),
                        metrics_enabled,
                    ),
                    None => config.evm_with_env(EmptyDB::default(), EthEvmEnv::default()),
                };
                evm.system_call(SystemTx::new(address, input)).unwrap().commit().status
            };

            // EVMs the engine did not wrap do not use the cache.
            let warm_input = Bytes::from_static(b"prewarmed");
            assert!(call(None, identity, warm_input.clone()));
            assert!(call(None, identity, warm_input.clone()));
            assert!(metrics_snapshot().is_empty());

            // Prewarming fills the shared cache without recording execution metrics.
            assert!(call(Some(false), identity, warm_input.clone()));
            assert!(metrics_snapshot().is_empty());

            assert!(call(Some(true), identity, warm_input.clone()));
            let new_input = Bytes::from_static(b"uncached");
            assert!(call(Some(true), identity, new_input.clone()));
            assert!(call(Some(true), identity, new_input));
            assert!(call(Some(true), sha256, warm_input.clone()));
            assert!(!call(Some(true), ecadd, Bytes::from(vec![0xff; 128])));

            let snapshot = metrics_snapshot();
            let counter = |address, name| {
                let label = format!("0x{address:02x}");
                let (_, _, _, value) = snapshot
                    .iter()
                    .find(|(key, ..)| {
                        key.key().name() == name &&
                            key.key()
                                .labels()
                                .any(|l| l.key() == "address" && l.value() == label)
                    })
                    .expect("address-labelled metric registered");
                value
            };
            assert_eq!(
                counter(identity, "sync.caching.precompile_cache_hits"),
                &DebugValue::Counter(2)
            );
            assert_eq!(
                counter(identity, "sync.caching.precompile_cache_misses"),
                &DebugValue::Counter(1)
            );
            assert_eq!(
                counter(sha256, "sync.caching.precompile_cache_misses"),
                &DebugValue::Counter(1)
            );
            assert_eq!(counter(ecadd, "sync.caching.precompile_errors"), &DebugValue::Counter(1));
            assert!(matches!(
                counter(identity, "sync.caching.precompile_cache_size"),
                DebugValue::Gauge(_)
            ));

            // An EVM created after the cache was filled is still uncached unless wrapped.
            assert!(call(None, identity, warm_input));
            // The debugging recorder resets values on each snapshot.
            assert!(metrics_snapshot().iter().all(|(.., value)| {
                value == &DebugValue::Counter(0) || value == &DebugValue::Gauge(0.0.into())
            }));
        });
    }
}
