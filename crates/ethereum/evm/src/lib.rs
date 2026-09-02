//! EVM config for vanilla ethereum.
//!
//! # Revm features
//!
//! This crate does __not__ enforce specific revm features such as `blst` or `c-kzg`, which are
//! critical for revm's evm internals, it is the responsibility of the implementer to ensure the
//! proper features are selected.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::{borrow::Cow, sync::Arc};
use alloy_consensus::Header;
use alloy_evm::{
    eth::{EthBlockExecutionCtx, EthBlockExecutorFactory},
    EthEvmFactory, FromRecoveredTx, FromTxWithEncoded,
};
#[cfg(feature = "jit")]
use core::any::Any;
use core::{convert::Infallible, fmt::Debug};
use reth_chainspec::{ChainSpec, EthChainSpec, MAINNET};
use reth_ethereum_primitives::{Block, EthPrimitives, TransactionSigned};
use reth_evm::{
    eth::NextEvmEnvAttributes, precompiles::PrecompilesMap, ConfigureEvm, EvmEnv, EvmFactory,
    JitBackend, NextBlockEnvAttributes, SenderRecoveryCache, TransactionEnvMut,
};
use reth_primitives_traits::{SealedBlock, SealedHeader};
use revm::{context::BlockEnv, primitives::hardfork::SpecId};

#[cfg(feature = "std")]
use reth_evm::{ConfigureEngineEvm, ExecutableTxIterator};
#[allow(unused_imports)]
use {
    alloy_eips::Decodable2718,
    alloy_primitives::{keccak256, Bytes, U256},
    alloy_rpc_types_engine::ExecutionData,
    reth_chainspec::EthereumHardforks,
    reth_evm::{EvmEnvFor, ExecutionCtxFor},
    reth_primitives_traits::{constants::MAX_TX_GAS_LIMIT_OSAKA, SignedTransaction, TxTy},
    reth_storage_errors::any::AnyError,
    revm::context::CfgEnv,
    revm::context_interface::block::BlobExcessGasAndPrice,
};

pub use alloy_evm::EthEvm;

mod config;
use alloy_evm::eth::spec::EthExecutorSpec;
pub use config::{revm_spec, revm_spec_by_timestamp_and_block_number};
use reth_ethereum_forks::Hardforks;

/// Helper type with backwards compatible methods to obtain Ethereum executor
/// providers.
#[doc(hidden)]
pub mod execute {
    use crate::EthEvmConfig;

    #[deprecated(note = "Use `EthEvmConfig` instead")]
    pub type EthExecutorProvider = EthEvmConfig;
}

mod build;
pub use build::EthBlockAssembler;

mod receipt;
pub use receipt::RethReceiptBuilder;

#[cfg(feature = "test-utils")]
mod test_utils;
#[cfg(feature = "test-utils")]
pub use test_utils::*;

pub mod factory;

/// Ethereum-related EVM configuration.
#[derive(Debug, Clone)]
pub struct EthEvmConfig<C = ChainSpec, EvmFactory = EthEvmFactory> {
    /// Inner [`EthBlockExecutorFactory`].
    pub executor_factory: EthBlockExecutorFactory<RethReceiptBuilder, Arc<C>, EvmFactory>,
    /// Ethereum block assembler.
    pub block_assembler: EthBlockAssembler<C>,
    /// Cache of recovered transaction senders, if enabled.
    pub sender_recovery_cache: Option<SenderRecoveryCache>,
}

impl EthEvmConfig {
    /// Creates a new Ethereum EVM configuration for the ethereum mainnet.
    pub fn mainnet() -> Self {
        Self::ethereum(MAINNET.clone())
    }
}

impl<ChainSpec> EthEvmConfig<ChainSpec> {
    /// Creates a new Ethereum EVM configuration with the given chain spec.
    pub fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self::ethereum(chain_spec)
    }

    /// Creates a new Ethereum EVM configuration.
    pub fn ethereum(chain_spec: Arc<ChainSpec>) -> Self {
        Self::new_with_evm_factory(chain_spec, EthEvmFactory::default())
    }
}

impl<ChainSpec, EvmFactory> EthEvmConfig<ChainSpec, EvmFactory> {
    /// Creates a new Ethereum EVM configuration with the given chain spec and EVM factory.
    pub fn new_with_evm_factory(chain_spec: Arc<ChainSpec>, evm_factory: EvmFactory) -> Self {
        Self {
            block_assembler: EthBlockAssembler::new(chain_spec.clone()),
            sender_recovery_cache: None,
            executor_factory: EthBlockExecutorFactory::new(
                RethReceiptBuilder::default(),
                chain_spec,
                evm_factory,
            ),
        }
    }

    /// Returns the chain spec associated with this configuration.
    pub const fn chain_spec(&self) -> &Arc<ChainSpec> {
        self.executor_factory.spec()
    }

    /// Uses the provided sender recovery cache.
    pub fn with_sender_recovery_cache(mut self, cache: SenderRecoveryCache) -> Self {
        self.sender_recovery_cache = Some(cache);
        self
    }
}

impl<ChainSpec, EvmF> ConfigureEvm for EthEvmConfig<ChainSpec, EvmF>
where
    ChainSpec: EthExecutorSpec + EthChainSpec<Header = Header> + Hardforks + 'static,
    EvmF: EvmFactory<
            Tx: TransactionEnvMut
                    + FromRecoveredTx<TransactionSigned>
                    + FromTxWithEncoded<TransactionSigned>,
            Spec = SpecId,
            BlockEnv = BlockEnv,
            Precompiles = PrecompilesMap,
        > + Clone
        + Debug
        + Send
        + Sync
        + Unpin
        + 'static,
{
    type Primitives = EthPrimitives;
    type Error = Infallible;
    type NextBlockEnvCtx = NextBlockEnvAttributes;
    type BlockExecutorFactory = EthBlockExecutorFactory<RethReceiptBuilder, Arc<ChainSpec>, EvmF>;
    type BlockAssembler = EthBlockAssembler<ChainSpec>;

    fn block_executor_factory(&self) -> &Self::BlockExecutorFactory {
        &self.executor_factory
    }

    fn block_assembler(&self) -> &Self::BlockAssembler {
        &self.block_assembler
    }

    fn with_jit_support_enabled(self, enabled: bool) -> Self
    where
        Self: Sized,
    {
        #[cfg(feature = "jit")]
        {
            let mut this = self;
            let mut evm_factory = this.executor_factory.evm_factory().clone();
            if let Some(factory) =
                (&mut evm_factory as &mut dyn Any).downcast_mut::<factory::RethEvmFactory>()
            {
                factory.set_jit_support(enabled);
            }
            this.executor_factory = EthBlockExecutorFactory::new(
                *this.executor_factory.receipt_builder(),
                this.executor_factory.spec().clone(),
                evm_factory,
            );
            this
        }

        #[cfg(not(feature = "jit"))]
        {
            let _ = enabled;
            self
        }
    }

    fn jit_backend(&self) -> Option<&dyn JitBackend> {
        #[cfg(feature = "jit")]
        if let Some(factory) = (self.executor_factory.evm_factory() as &dyn Any)
            .downcast_ref::<factory::RethEvmFactory>()
        {
            return Some(factory);
        }

        None
    }

    fn evm_env(&self, header: &Header) -> Result<EvmEnv<SpecId>, Self::Error> {
        Ok(EvmEnv::for_eth_block(
            header,
            self.chain_spec(),
            self.chain_spec().chain().id(),
            self.chain_spec().blob_params_at_timestamp(header.timestamp),
        ))
    }

    fn next_evm_env(
        &self,
        parent: &Header,
        attributes: &NextBlockEnvAttributes,
    ) -> Result<EvmEnv, Self::Error> {
        Ok(EvmEnv::for_eth_next_block(
            parent,
            NextEvmEnvAttributes {
                timestamp: attributes.timestamp,
                suggested_fee_recipient: attributes.suggested_fee_recipient,
                prev_randao: attributes.prev_randao,
                gas_limit: attributes.gas_limit,
                slot_number: attributes.slot_number,
            },
            self.chain_spec().next_block_base_fee(parent, attributes.timestamp).unwrap_or_default(),
            self.chain_spec(),
            self.chain_spec().chain().id(),
            self.chain_spec().blob_params_at_timestamp(attributes.timestamp),
        ))
    }

    fn context_for_block<'a>(
        &self,
        block: &'a SealedBlock<Block>,
    ) -> Result<EthBlockExecutionCtx<'a>, Self::Error> {
        Ok(EthBlockExecutionCtx {
            tx_count_hint: Some(block.transaction_count()),
            parent_hash: block.header().parent_hash,
            parent_beacon_block_root: block.header().parent_beacon_block_root,
            ommers: &block.body().ommers,
            withdrawals: block.body().withdrawals.as_ref().map(|w| Cow::Borrowed(w.as_slice())),
            extra_data: block.header().extra_data.clone(),
            slot_number: block.header().slot_number,
        })
    }

    fn context_for_next_block(
        &self,
        parent: &SealedHeader,
        attributes: Self::NextBlockEnvCtx,
    ) -> Result<EthBlockExecutionCtx<'_>, Self::Error> {
        Ok(EthBlockExecutionCtx {
            tx_count_hint: None,
            parent_hash: parent.hash(),
            parent_beacon_block_root: attributes.parent_beacon_block_root,
            ommers: &[],
            withdrawals: attributes.withdrawals.map(|w| Cow::Owned(w.into_inner())),
            extra_data: attributes.extra_data,
            slot_number: attributes.slot_number,
        })
    }
}

#[cfg(feature = "std")]
impl<ChainSpec, EvmF> ConfigureEngineEvm<ExecutionData> for EthEvmConfig<ChainSpec, EvmF>
where
    ChainSpec: EthExecutorSpec + EthChainSpec<Header = Header> + Hardforks + 'static,
    EvmF: EvmFactory<
            Tx: TransactionEnvMut
                    + FromRecoveredTx<TransactionSigned>
                    + FromTxWithEncoded<TransactionSigned>,
            Spec = SpecId,
            BlockEnv = BlockEnv,
            Precompiles = PrecompilesMap,
        > + Clone
        + Debug
        + Send
        + Sync
        + Unpin
        + 'static,
{
    fn evm_env_for_payload(&self, payload: &ExecutionData) -> Result<EvmEnvFor<Self>, Self::Error> {
        let timestamp = payload.payload.timestamp();
        let block_number = payload.payload.block_number();

        let blob_params = self.chain_spec().blob_params_at_timestamp(timestamp);
        let spec =
            revm_spec_by_timestamp_and_block_number(self.chain_spec(), timestamp, block_number);

        // configure evm env based on parent block
        let mut cfg_env = CfgEnv::new()
            .with_chain_id(self.chain_spec().chain().id())
            .with_spec_and_mainnet_gas_params(spec);

        if let Some(blob_params) = &blob_params {
            cfg_env.set_max_blobs_per_tx(blob_params.max_blobs_per_tx);
        }

        if self.chain_spec().is_osaka_active_at_timestamp(timestamp) {
            cfg_env.tx_gas_limit_cap = Some(MAX_TX_GAS_LIMIT_OSAKA);
        }

        // derive the EIP-4844 blob fees from the header's `excess_blob_gas` and the current
        // blobparams
        let blob_excess_gas_and_price =
            payload.payload.excess_blob_gas().zip(blob_params).map(|(excess_blob_gas, params)| {
                let blob_gasprice = params.calc_blob_fee(excess_blob_gas);
                BlobExcessGasAndPrice { excess_blob_gas, blob_gasprice }
            });

        let block_env = BlockEnv {
            number: U256::from(block_number),
            beneficiary: payload.payload.fee_recipient(),
            timestamp: U256::from(timestamp),
            difficulty: if spec >= SpecId::MERGE {
                U256::ZERO
            } else {
                payload.payload.as_v1().prev_randao.into()
            },
            prevrandao: (spec >= SpecId::MERGE).then(|| payload.payload.as_v1().prev_randao),
            gas_limit: payload.payload.gas_limit(),
            basefee: payload.payload.saturated_base_fee_per_gas(),
            blob_excess_gas_and_price,
            slot_num: payload.payload.as_v4().map(|v4| v4.slot_number).unwrap_or_default(),
        };

        Ok(EvmEnv { cfg_env, block_env })
    }

    fn context_for_payload<'a>(
        &self,
        payload: &'a ExecutionData,
    ) -> Result<ExecutionCtxFor<'a, Self>, Self::Error> {
        Ok(EthBlockExecutionCtx {
            tx_count_hint: Some(payload.payload.transactions().len()),
            parent_hash: payload.parent_hash(),
            parent_beacon_block_root: payload.sidecar.parent_beacon_block_root(),
            ommers: &[],
            withdrawals: payload.payload.withdrawals().map(|w| Cow::Borrowed(w.as_slice())),
            extra_data: payload.payload.as_v1().extra_data.clone(),
            slot_number: payload.payload.as_v4().map(|v4| v4.slot_number),
        })
    }

    fn tx_iterator_for_payload(
        &self,
        payload: &ExecutionData,
    ) -> Result<impl ExecutableTxIterator<Self>, Self::Error> {
        let txs = payload.payload.transactions().clone();
        let sender_recovery_cache = self.sender_recovery_cache.clone();
        let convert = move |raw: Bytes| {
            let tx =
                TxTy::<Self::Primitives>::decode_2718_exact(raw.as_ref()).map_err(AnyError::new)?;
            let tx = with_hash_from_raw(tx, &raw);
            let signer = if let Some(cache) = &sender_recovery_cache {
                cache.recover(&tx)
            } else {
                tx.try_recover()
            }
            .map_err(AnyError::new)?;
            Ok::<_, AnyError>(tx.with_signer(signer))
        };

        Ok((txs, convert))
    }
}

/// Returns `tx` with its hash cache populated from `raw`, the buffer it was decoded from, saving
/// the re-encode that the first `tx_hash()` call would otherwise perform.
///
/// [`Decodable2718::decode_2718_exact`] rejects non-canonical RLP and trailing bytes, so
/// re-encoding `tx` reproduces `raw` and `keccak256(raw)` is the transaction hash. The one input
/// it normalizes is a legacy transaction carrying a `0x00` type prefix, which re-encodes untagged;
/// that case keeps the lazily computed hash.
#[cfg(feature = "std")]
fn with_hash_from_raw(tx: TransactionSigned, raw: &[u8]) -> TransactionSigned {
    if raw.first() == Some(&0) {
        return tx;
    }

    let signed = tx.into_signed();
    let signature = *signed.signature();
    TransactionSigned::new_unchecked(signed.strip_signature(), signature, keccak256(raw))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{
        Header, SignableTransaction, TxEip1559, TxEip2930, TxEip4844, TxEip7702, TxLegacy,
    };
    use alloy_eips::{
        eip2930::{AccessList, AccessListItem},
        eip7702::{Authorization, SignedAuthorization},
        Encodable2718,
    };
    use alloy_genesis::Genesis;
    use alloy_primitives::{Address, Signature, TxKind, B256};
    use reth_chainspec::{Chain, ChainSpec};
    use reth_evm::{execute::ProviderError, EvmEnv};
    use revm::{
        context::{BlockEnv, CfgEnv},
        database::CacheDB,
        database_interface::EmptyDBTyped,
        inspector::NoOpInspector,
    };

    /// Builds one transaction of each type, all sharing the same dummy signature.
    fn all_tx_types() -> Vec<TransactionSigned> {
        let signature = Signature::new(U256::from(1), U256::from(2), false);
        let to = Address::repeat_byte(0x33);
        let value = U256::from(5);
        let input = Bytes::from_static(&[0xab, 0xcd]);
        let access_list = AccessList(vec![AccessListItem {
            address: Address::repeat_byte(0x11),
            storage_keys: vec![B256::repeat_byte(0x22)],
        }]);

        vec![
            TxLegacy {
                chain_id: Some(1),
                nonce: 2,
                gas_price: 3,
                gas_limit: 4,
                to: TxKind::Call(to),
                value,
                input: input.clone(),
            }
            .into_signed(signature)
            .into(),
            TxEip2930 {
                chain_id: 1,
                nonce: 2,
                gas_price: 3,
                gas_limit: 4,
                to: TxKind::Call(to),
                value,
                access_list: access_list.clone(),
                input: input.clone(),
            }
            .into_signed(signature)
            .into(),
            TxEip1559 {
                chain_id: 1,
                nonce: 2,
                gas_limit: 4,
                max_fee_per_gas: 5,
                max_priority_fee_per_gas: 6,
                to: TxKind::Call(to),
                value,
                access_list: access_list.clone(),
                input: input.clone(),
            }
            .into_signed(signature)
            .into(),
            TxEip4844 {
                chain_id: 1,
                nonce: 2,
                gas_limit: 4,
                max_fee_per_gas: 5,
                max_priority_fee_per_gas: 6,
                to,
                value,
                access_list: access_list.clone(),
                blob_versioned_hashes: vec![B256::repeat_byte(0x44)],
                max_fee_per_blob_gas: 7,
                input: input.clone(),
            }
            .into_signed(signature)
            .into(),
            TxEip7702 {
                chain_id: 1,
                nonce: 2,
                gas_limit: 4,
                max_fee_per_gas: 5,
                max_priority_fee_per_gas: 6,
                to,
                value,
                access_list,
                authorization_list: vec![SignedAuthorization::new_unchecked(
                    Authorization {
                        chain_id: U256::from(1),
                        address: Address::repeat_byte(0x55),
                        nonce: 8,
                    },
                    1,
                    U256::from(9),
                    U256::from(10),
                )],
                input,
            }
            .into_signed(signature)
            .into(),
        ]
    }

    #[test]
    fn tx_hash_seeded_from_raw_bytes() {
        for tx in all_tx_types() {
            let raw = Bytes::from(tx.encoded_2718());
            let decoded = TransactionSigned::decode_2718_exact(raw.as_ref()).unwrap();
            let seeded = with_hash_from_raw(decoded, &raw);

            assert_eq!(*seeded.tx_hash(), keccak256(&raw), "{:?}", tx.tx_type());
            assert_eq!(seeded.tx_hash(), tx.tx_hash(), "{:?}", tx.tx_type());
        }
    }

    #[test]
    fn tx_hash_is_taken_from_the_raw_bytes() {
        // the returned transaction must carry the supplied hash instead of lazily recomputing it
        let raw = Bytes::from_static(b"not a transaction encoding");
        let seeded = with_hash_from_raw(all_tx_types().remove(0), &raw);

        assert_eq!(*seeded.tx_hash(), keccak256(&raw));
    }

    #[test]
    fn tx_hash_not_seeded_for_type_prefixed_legacy() {
        let legacy = all_tx_types().remove(0);
        let mut raw = vec![0x00];
        raw.extend_from_slice(&legacy.encoded_2718());

        // the decoder strips the `0x00` prefix, so the raw bytes are not the hash preimage
        let decoded = TransactionSigned::decode_2718_exact(&raw).unwrap();
        let seeded = with_hash_from_raw(decoded, &raw);

        assert_eq!(seeded.tx_hash(), legacy.tx_hash());
        assert_ne!(*seeded.tx_hash(), keccak256(&raw));
    }

    #[test]
    fn test_fill_cfg_and_block_env() {
        // Create a default header
        let header = Header::default();

        // Build the ChainSpec for Ethereum mainnet, activating London, Paris, and Shanghai
        // hardforks
        let chain_spec = ChainSpec::builder()
            .chain(Chain::mainnet())
            .genesis(Genesis::default())
            .london_activated()
            .paris_activated()
            .shanghai_activated()
            .build();

        // Use the `EthEvmConfig` to fill the `cfg_env` and `block_env` based on the ChainSpec,
        // Header, and total difficulty
        let EvmEnv { cfg_env, .. } =
            EthEvmConfig::new(Arc::new(chain_spec.clone())).evm_env(&header).unwrap();

        // Assert that the chain ID in the `cfg_env` is correctly set to the chain ID of the
        // ChainSpec
        assert_eq!(cfg_env.chain_id, chain_spec.chain().id());
    }

    #[test]
    fn test_evm_with_env_default_spec() {
        let evm_config = EthEvmConfig::mainnet();

        let db = CacheDB::<EmptyDBTyped<ProviderError>>::default();

        let evm_env = EvmEnv::default();

        let evm = evm_config.evm_with_env(db, evm_env.clone());

        // Check that the EVM environment
        assert_eq!(evm.block, evm_env.block_env);
        assert_eq!(evm.cfg, evm_env.cfg_env);
    }

    #[test]
    fn test_evm_with_env_custom_cfg() {
        let evm_config = EthEvmConfig::mainnet();

        let db = CacheDB::<EmptyDBTyped<ProviderError>>::default();

        // Create a custom configuration environment with a chain ID of 111
        let cfg = CfgEnv::default().with_chain_id(111);

        let evm_env = EvmEnv { cfg_env: cfg.clone(), ..Default::default() };

        let evm = evm_config.evm_with_env(db, evm_env);

        // Check that the EVM environment is initialized with the custom environment
        assert_eq!(evm.cfg, cfg);
    }

    #[test]
    fn test_evm_with_env_custom_block_and_tx() {
        let evm_config = EthEvmConfig::mainnet();

        let db = CacheDB::<EmptyDBTyped<ProviderError>>::default();

        // Create customs block and tx env
        let block = BlockEnv {
            basefee: 1000,
            gas_limit: 10_000_000,
            number: U256::from(42),
            ..Default::default()
        };

        let evm_env = EvmEnv { block_env: block, ..Default::default() };

        let evm = evm_config.evm_with_env(db, evm_env.clone());

        // Verify that the block and transaction environments are set correctly
        assert_eq!(evm.block, evm_env.block_env);

        // Default spec ID
        assert_eq!(evm.cfg.spec, SpecId::default());
    }

    #[test]
    fn test_evm_with_spec_id() {
        let evm_config = EthEvmConfig::mainnet();

        let db = CacheDB::<EmptyDBTyped<ProviderError>>::default();

        let evm_env = EvmEnv {
            cfg_env: CfgEnv::new().with_spec_and_mainnet_gas_params(SpecId::PETERSBURG),
            ..Default::default()
        };

        let evm = evm_config.evm_with_env(db, evm_env);

        // Check that the spec ID is setup properly
        assert_eq!(evm.cfg.spec, SpecId::PETERSBURG);
    }

    #[test]
    fn test_evm_with_env_and_default_inspector() {
        let evm_config = EthEvmConfig::mainnet();
        let db = CacheDB::<EmptyDBTyped<ProviderError>>::default();

        let evm_env = EvmEnv::default();

        let evm = evm_config.evm_with_env_and_inspector(db, evm_env.clone(), NoOpInspector {});

        // Check that the EVM environment is set to default values
        assert_eq!(evm.block, evm_env.block_env);
        assert_eq!(evm.cfg, evm_env.cfg_env);
    }

    #[test]
    fn test_evm_with_env_inspector_and_custom_cfg() {
        let evm_config = EthEvmConfig::mainnet();
        let db = CacheDB::<EmptyDBTyped<ProviderError>>::default();

        let cfg_env = CfgEnv::default().with_chain_id(111);
        let block = BlockEnv::default();
        let evm_env = EvmEnv { cfg_env: cfg_env.clone(), block_env: block };

        let evm = evm_config.evm_with_env_and_inspector(db, evm_env, NoOpInspector {});

        // Check that the EVM environment is set with custom configuration
        assert_eq!(evm.cfg, cfg_env);
        assert_eq!(evm.cfg.spec, SpecId::default());
    }

    #[test]
    fn test_evm_with_env_inspector_and_custom_block_tx() {
        let evm_config = EthEvmConfig::mainnet();
        let db = CacheDB::<EmptyDBTyped<ProviderError>>::default();

        // Create custom block and tx environment
        let block = BlockEnv {
            basefee: 1000,
            gas_limit: 10_000_000,
            number: U256::from(42),
            ..Default::default()
        };
        let evm_env = EvmEnv { block_env: block, ..Default::default() };

        let evm = evm_config.evm_with_env_and_inspector(db, evm_env.clone(), NoOpInspector {});

        // Verify that the block and transaction environments are set correctly
        assert_eq!(evm.block, evm_env.block_env);
        assert_eq!(evm.cfg.spec, SpecId::default());
    }

    #[test]
    fn test_evm_with_env_inspector_and_spec_id() {
        let evm_config = EthEvmConfig::mainnet();
        let db = CacheDB::<EmptyDBTyped<ProviderError>>::default();

        let evm_env = EvmEnv {
            cfg_env: CfgEnv::new().with_spec_and_mainnet_gas_params(SpecId::PETERSBURG),
            ..Default::default()
        };

        let evm = evm_config.evm_with_env_and_inspector(db, evm_env.clone(), NoOpInspector {});

        // Check that the spec ID is set properly
        assert_eq!(evm.block, evm_env.block_env);
        assert_eq!(evm.cfg, evm_env.cfg_env);
        assert_eq!(evm.tx, Default::default());
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_support_downcast_updates_reth_factory() {
        let evm_config = EthEvmConfig::new_with_evm_factory(
            MAINNET.clone(),
            factory::RethEvmFactory::disabled(),
        );

        assert!(evm_config.jit_backend().is_some());
        assert!(!evm_config.executor_factory.evm_factory().jit_support_enabled());

        let evm_config = evm_config.with_jit_support();
        assert!(evm_config.executor_factory.evm_factory().jit_support_enabled());

        let evm_config = evm_config.with_jit_support_enabled(false);
        assert!(!evm_config.executor_factory.evm_factory().jit_support_enabled());
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_support_downcast_ignores_plain_factory() {
        let evm_config = EthEvmConfig::mainnet();

        assert!(evm_config.jit_backend().is_none());

        let evm_config = evm_config.with_jit_support();
        assert!(evm_config.jit_backend().is_none());
    }
}
