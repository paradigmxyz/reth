#![cfg_attr(test, allow(dead_code))]
pub mod header;


#[cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::sync::Arc;

use core::convert::Infallible;
use alloy_consensus::Header;
use alloy_consensus::{BlockHeader as _, Transaction as _};
use alloy_primitives::U256;
use reth_evm::{ConfigureEvm, EvmEnv};
use alloy_eips::Decodable2718;
use reth_primitives_traits::SignedTransaction;
use reth_evm::{ConfigureEngineEvm, EvmEnvFor, ExecutableTxIterator, ExecutionCtxFor};
use reth_primitives_traits::{TxTy, WithEncoded, SealedBlock, SealedHeader, NodePrimitives, BlockTy, BlockHeader};
use alloy_evm::tx::{FromRecoveredTx, FromTxWithEncoded};
use alloy_consensus::{BlockHeader as _, Transaction as _};
use reth_storage_errors::any::AnyError;
use reth_arbitrum_payload::ArbExecutionData;
use reth_arbitrum_chainspec::ArbitrumChainSpec;
use revm::{
    context::{BlockEnv, CfgEnv},
    context_interface::block::BlobExcessGasAndPrice,
    primitives::hardfork::SpecId,
};

mod config;
pub use config::{ArbBlockAssembler, ArbNextBlockEnvAttributes};

mod build;
pub use build::{ArbBlockExecutionCtx, ArbBlockExecutorFactory};
pub mod execute;

pub mod receipts;
pub use receipts::*;
mod predeploys;
pub use predeploys::*;
mod retryables;
pub use retryables::*;
mod arb_evm;
pub use arb_evm::{ArbTransaction, ArbEvm, ArbEvmFactory};

mod log_sink;
pub mod storage;
pub mod l1_pricing;
pub mod l2_pricing;
pub mod addressset;
pub mod addresstable;
pub mod merkleaccumulator;
pub mod blockhash;
pub mod features;
pub mod programs;
pub mod arbosstate;
mod internal_tx;


pub struct ArbEvmConfig<ChainSpec = (), N = (), R = ArbRethReceiptBuilder>
where
    R: Clone, {
    pub executor_factory: ArbBlockExecutorFactory<R, ChainSpec>,
    pub block_assembler: ArbBlockAssembler<ChainSpec>,
    _pd: core::marker::PhantomData<N>,
}

impl<ChainSpec, N: Clone, R: Clone> Clone for ArbEvmConfig<ChainSpec, N, R> {
    fn clone(&self) -> Self {
        Self {
            executor_factory: self.executor_factory.clone(),
            block_assembler: self.block_assembler.clone(),
            _pd: self._pd.clone(),
        }
    }
}

impl<ChainSpec, N, R: Clone> core::fmt::Debug for ArbEvmConfig<ChainSpec, N, R> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ArbEvmConfig").finish()
}
}


    impl<ChainSpec, N, R> Default for ArbEvmConfig<ChainSpec, N, R>
where
    ChainSpec: Default,
    R: Default + Clone,
{
    fn default() -> Self {
        let cs = Arc::new(ChainSpec::default());
        Self::new(cs, R::default())
    }
}

impl<ChainSpec, N, R> ArbEvmConfig<ChainSpec, N, R>
where
    R: Clone,
{
    pub fn new(chain_spec: Arc<ChainSpec>, receipt_builder: R) -> Self {
        Self {
            block_assembler: ArbBlockAssembler::new(chain_spec.clone()),
            executor_factory: ArbBlockExecutorFactory::new(receipt_builder, chain_spec),
            _pd: core::marker::PhantomData,
        }
    }

    pub const fn chain_spec(&self) -> &Arc<ChainSpec> {
        self.executor_factory.spec()
    }

}

impl<ChainSpec, N, R> ConfigureEvm for ArbEvmConfig<ChainSpec, N, R>
where
    ChainSpec: ArbitrumChainSpec + Send + Sync + Unpin + core::fmt::Debug + 'static,
    N: reth_primitives_traits::NodePrimitives<
        SignedTx = reth_arbitrum_primitives::ArbTransactionSigned,
        Receipt = reth_arbitrum_primitives::ArbReceipt,
        Block = alloy_consensus::Block<reth_arbitrum_primitives::ArbTransactionSigned>
    > + Clone + Send + Sync + Unpin + core::fmt::Debug,
    R: Clone + Send + Sync + Unpin + core::fmt::Debug + 'static + alloy_evm::eth::receipt_builder::ReceiptBuilder<Transaction = reth_arbitrum_primitives::ArbTransactionSigned, Receipt = reth_arbitrum_primitives::ArbReceipt>,
{
    type Primitives = N;
    type Error = core::convert::Infallible;
    type NextBlockEnvCtx = ArbNextBlockEnvAttributes;
    type BlockExecutorFactory = ArbBlockExecutorFactory<R, ChainSpec>;
    type BlockAssembler = ArbBlockAssembler<ChainSpec>;

    fn block_executor_factory(&self) -> &Self::BlockExecutorFactory {
        &self.executor_factory
    }

    fn block_assembler(&self) -> &Self::BlockAssembler {
        &self.block_assembler
    }

    fn evm_env(&self, header: &<N as NodePrimitives>::BlockHeader) -> Result<EvmEnv<SpecId>, Infallible> {
        let chain_id = self.chain_spec().chain_id() as u64;
        let spec = self.chain_spec().spec_id_by_timestamp(header.timestamp());
        let mut cfg_env = CfgEnv::new().with_chain_id(chain_id).with_spec(spec);
        let block_env = BlockEnv {
            number: U256::from(header.number()),
            beneficiary: header.beneficiary(),
            timestamp: U256::from(header.timestamp()),
            difficulty: header.difficulty(),
            prevrandao: header.mix_hash(),
            gas_limit: header.gas_limit(),
            basefee: header.base_fee_per_gas().unwrap_or_default(),
            blob_excess_gas_and_price: Some(BlobExcessGasAndPrice { excess_blob_gas: 0, blob_gasprice: 0 }),
        };
        Ok(EvmEnv { cfg_env, block_env })
    }

    fn next_evm_env(
        &self,
        parent: &<N as NodePrimitives>::BlockHeader,
        attributes: &Self::NextBlockEnvCtx,
    ) -> Result<EvmEnv<SpecId>, Self::Error> {
        let chain_id = self.chain_spec().chain_id() as u64;
        let spec = self.chain_spec().spec_id_by_timestamp(attributes.timestamp);
        let mut cfg_env = CfgEnv::new().with_chain_id(chain_id).with_spec(spec);
        let next_number = parent.number().saturating_add(1);
        let block_env = BlockEnv {
            number: U256::from(next_number),
            beneficiary: attributes.suggested_fee_recipient,
            timestamp: U256::from(attributes.timestamp),
            difficulty: U256::from(1),
            prevrandao: Some(attributes.prev_randao),
            gas_limit: attributes.gas_limit,
            basefee: attributes.max_fee_per_gas.unwrap_or_default().try_into().unwrap_or_default(),
            blob_excess_gas_and_price: Some(BlobExcessGasAndPrice { excess_blob_gas: 0, blob_gasprice: 0 }),
        };
        Ok(EvmEnv { cfg_env, block_env })
    }
    fn context_for_block(&self, block: &'_ SealedBlock<BlockTy<Self::Primitives>>) -> Result<ArbBlockExecutionCtx, Infallible> {
        Ok(ArbBlockExecutionCtx {
            parent_hash: block.header().parent_hash,
            parent_beacon_block_root: block.header().parent_beacon_block_root,
            extra_data: block.header().extra_data.clone().into(),
            delayed_messages_read: 0,
            l1_block_number: 0,
        })
    }

    fn context_for_next_block(
        &self,
        parent: &SealedHeader<<Self::Primitives as reth_primitives_traits::NodePrimitives>::BlockHeader>,
        attributes: Self::NextBlockEnvCtx,
    ) -> Result<ArbBlockExecutionCtx, Infallible> {
        Ok(ArbBlockExecutionCtx {
            parent_hash: parent.hash(),
            parent_beacon_block_root: attributes.parent_beacon_block_root,
            extra_data: attributes.extra_data.into(),
            delayed_messages_read: attributes.delayed_messages_read,
            l1_block_number: attributes.l1_block_number,
        })
    }

}

impl<ChainSpec, N, R> ConfigureEngineEvm<ArbExecutionData> for ArbEvmConfig<ChainSpec, N, R>
where
    ChainSpec: ArbitrumChainSpec + Send + Sync + Unpin + core::fmt::Debug + 'static,
    N: reth_primitives_traits::NodePrimitives<
        SignedTx = reth_arbitrum_primitives::ArbTransactionSigned,
        Receipt = reth_arbitrum_primitives::ArbReceipt,
        Block = alloy_consensus::Block<reth_arbitrum_primitives::ArbTransactionSigned>
    > + Send + Sync + Unpin + core::fmt::Debug + Clone,
    R: Send + Sync + Unpin + core::fmt::Debug + Clone + 'static + alloy_evm::eth::receipt_builder::ReceiptBuilder<Transaction = reth_arbitrum_primitives::ArbTransactionSigned, Receipt = reth_arbitrum_primitives::ArbReceipt>,
{
    fn evm_env_for_payload(
        &self,
        payload: &ArbExecutionData,
    ) -> Result<EvmEnvFor<Self>, Infallible> {
        let chain_id = self.chain_spec().chain_id() as u64;
        let spec = self.chain_spec().spec_id_by_timestamp(payload.payload.timestamp());
        let mut cfg_env = CfgEnv::new().with_chain_id(chain_id).with_spec(spec);
        let block_env = BlockEnv {
            number: U256::from(payload.payload.block_number()),
            beneficiary: payload.payload.as_v1().fee_recipient,
            timestamp: U256::from(payload.payload.timestamp()),
            difficulty: U256::ZERO,
            prevrandao: Some(payload.payload.as_v1().prev_randao),
            gas_limit: payload.payload.as_v1().gas_limit,
            basefee: payload.payload.as_v1().base_fee_per_gas.try_into().unwrap_or_default(),
            blob_excess_gas_and_price: Some(BlobExcessGasAndPrice { excess_blob_gas: 0, blob_gasprice: 0 }),
        };
        Ok(EvmEnv { cfg_env, block_env })
    }

    fn context_for_payload<'a>(
        &self,
        payload: &'a ArbExecutionData,
    ) -> Result<ExecutionCtxFor<'a, Self>, Infallible> {
        Ok(ArbBlockExecutionCtx {
            parent_hash: payload.parent_hash(),
            parent_beacon_block_root: payload.sidecar.parent_beacon_block_root,
            extra_data: payload.payload.as_v1().extra_data.clone().into(),
            delayed_messages_read: 0,
            l1_block_number: 0,
        })
    }

    fn tx_iterator_for_payload(
        &self,
        payload: &ArbExecutionData,
    ) -> Result<impl ExecutableTxIterator<Self>, Infallible> {
        Ok(payload
            .payload
            .transactions()
            .clone()
            .into_iter()
            .map(|encoded| {
                let tx = reth_arbitrum_primitives::ArbTransactionSigned::decode_2718_exact(encoded.as_ref())
                    .map_err(AnyError::new)?;
                let signer = tx.try_recover().map_err(AnyError::new)?;
                Ok::<_, AnyError>(WithEncoded::new(encoded.into(), tx.with_signer(signer)))
            }))
    }
}
impl<ChainSpec: ArbitrumChainSpec, N, R: Clone> ArbEvmConfig<ChainSpec, N, R> {
    pub fn decode_arb_envelope(
        &self,
        bytes: &[u8],
    ) -> Result<arb_alloy_consensus::ArbTxEnvelope, AnyError> {
        let (env, _) = arb_alloy_consensus::ArbTxEnvelope::decode_typed(bytes)
            .map_err(AnyError::new)?;
        Ok(env)
    }
}
impl<ChainSpec: ArbitrumChainSpec, N, R: Clone> ArbEvmConfig<ChainSpec, N, R> {
    pub fn tx_envelopes_for_payload(
        &self,
        payload: &ArbExecutionData,
    ) -> impl Iterator<Item = Result<(alloy_primitives::Bytes, arb_alloy_consensus::ArbTxEnvelope), AnyError>> + '_ {
        payload
            .payload
            .transactions()
            .clone()
            .into_iter()
            .map(|encoded| {
                let (env, _) = arb_alloy_consensus::ArbTxEnvelope::decode_typed(encoded.as_ref())
                    .map_err(AnyError::new)?;
                Ok((encoded, env))
            })
    }
}


impl<ChainSpec: ArbitrumChainSpec, N, R: Clone> ArbEvmConfig<ChainSpec, N, R> {
    pub fn default_predeploy_registry(&self) -> PredeployRegistry {
        PredeployRegistry::default()
    }

    pub fn arb_tx_iterator_for_payload(
        &self,
        payload: &ArbExecutionData,
    ) -> impl Iterator<Item = Result<(alloy_primitives::Bytes, reth_arbitrum_primitives::ArbTransactionSigned), AnyError>> + '_ {
        payload
            .payload
            .transactions()
            .clone()
            .into_iter()
            .map(|encoded| {
                let tx = reth_arbitrum_primitives::ArbTransactionSigned::decode_2718_exact(encoded.as_ref())
                    .map_err(AnyError::new)?;
                Ok::<_, AnyError>((encoded, tx))
            })
    }

    pub fn map_env_to_tx_type(env: &arb_alloy_consensus::ArbTxEnvelope) -> reth_arbitrum_primitives::ArbTxType {
        match env {
            arb_alloy_consensus::ArbTxEnvelope::Deposit(_) => reth_arbitrum_primitives::ArbTxType::Deposit,
            arb_alloy_consensus::ArbTxEnvelope::Unsigned(_) => reth_arbitrum_primitives::ArbTxType::Unsigned,
            arb_alloy_consensus::ArbTxEnvelope::Contract(_) => reth_arbitrum_primitives::ArbTxType::Contract,
            arb_alloy_consensus::ArbTxEnvelope::Retry(_) => reth_arbitrum_primitives::ArbTxType::Retry,
            arb_alloy_consensus::ArbTxEnvelope::SubmitRetryable(_) => reth_arbitrum_primitives::ArbTxType::SubmitRetryable,
            arb_alloy_consensus::ArbTxEnvelope::Internal(_) => reth_arbitrum_primitives::ArbTxType::Internal,
            arb_alloy_consensus::ArbTxEnvelope::Legacy(_) => reth_arbitrum_primitives::ArbTxType::Legacy,
        }
    }
}





pub(crate) mod tests {
    use alloy_evm::tx::RecoveredTx;
pub(crate) mod test_helpers {
    use super::*;
    #[derive(Clone, Debug, Default)]
    pub struct TestChainSpec;
    impl ArbitrumChainSpec for TestChainSpec {

        fn chain_id(&self) -> u64 { 42161 }
        fn spec_id_by_timestamp(&self, _ts: u64) -> SpecId { SpecId::CANCUN }
    }
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct TestPrims;
    impl reth_primitives_traits::NodePrimitives for TestPrims {
        type BlockHeader = alloy_consensus::Header;
        type Block = alloy_consensus::Block<reth_arbitrum_primitives::ArbTransactionSigned>;
        type BlockBody = alloy_consensus::BlockBody<reth_arbitrum_primitives::ArbTransactionSigned>;
        type Receipt = reth_arbitrum_primitives::ArbReceipt;
        type SignedTx = reth_arbitrum_primitives::ArbTransactionSigned;
    }
}

    use super::*;
    use alloy_primitives::Address;

    #[test]
    fn arb_evm_config_default_constructs() {
        let _cfg = ArbEvmConfig::<crate::tests::test_helpers::TestChainSpec, crate::tests::test_helpers::TestPrims, ArbRethReceiptBuilder>::default();
    }

    #[test]
    fn arb_block_assembler_and_factory_construct() {
        let cs = alloc::sync::Arc::new(());
        let _asm = ArbBlockAssembler::new(cs.clone());
        let _fac = ArbBlockExecutorFactory::new(ArbRethReceiptBuilder, cs);
    }

    #[test]
    fn decode_arb_envelope_deposit_roundtrip() {
        use arb_alloy_consensus::tx::ArbDepositTx;
        use alloy_primitives::{address, B256, U256};
        let dep = ArbDepositTx {
            chain_id: U256::from(42161u64),
            l1_request_id: B256::from([0x11u8; 32]),
            from: address!("00000000000000000000000000000000000000aa"),
            to: address!("00000000000000000000000000000000000000bb"),
            value: U256::from(12345u64),
        };
        let env = arb_alloy_consensus::ArbTxEnvelope::Deposit(dep.clone());
        let encoded = env.encode_typed();
        let cfg = ArbEvmConfig::<crate::tests::test_helpers::TestChainSpec, crate::tests::test_helpers::TestPrims, ArbRethReceiptBuilder>::default();
        let decoded = cfg.decode_arb_envelope(&encoded).expect("decode ok");
        match decoded {
            arb_alloy_consensus::ArbTxEnvelope::Deposit(inner) => {
                assert_eq!(inner.chain_id, dep.chain_id);
                assert_eq!(inner.l1_request_id, dep.l1_request_id);
                assert_eq!(inner.from, dep.from);
                assert_eq!(inner.to, dep.to);
                assert_eq!(inner.value, dep.value);
            }
            _ => panic!("expected deposit envelope"),
        }
    }
    #[test]
    fn default_predeploy_registry_dispatches_known_address() {
        use alloy_primitives::address;
        let cfg = ArbEvmConfig::<crate::tests::test_helpers::TestChainSpec, crate::tests::test_helpers::TestPrims, ArbRethReceiptBuilder>::default();
        let mut reg = cfg.default_predeploy_registry();
        let sys = address!("0000000000000000000000000000000000000064");
        let ctx = crate::predeploys::PredeployCallContext {
            block_number: 100,
            block_hashes: alloc::vec::Vec::new(),
            chain_id: U256::from(42161u64),
            os_version: 0,
            time: 0,
            origin: alloy_primitives::Address::ZERO,
            caller: alloy_primitives::Address::ZERO,
            depth: 1,
            basefee: U256::ZERO,
        };
        let out = reg.dispatch(&ctx, sys, &alloy_primitives::Bytes::default(), 21_000, U256::ZERO);
        assert!(out.is_some());
    }
    #[test]
    fn arb_tx_iterator_maps_envelope_types() {
        use arb_alloy_consensus::tx::{ArbDepositTx, ArbUnsignedTx};
        use alloy_primitives::{address, b256, Bytes, U256};
        let dep = arb_alloy_consensus::ArbTxEnvelope::Deposit(ArbDepositTx {
            chain_id: U256::from(42161u64),
            l1_request_id: b256!("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            from: address!("00000000000000000000000000000000000000aa"),
            to: address!("00000000000000000000000000000000000000bb"),
            value: U256::from(1u64),
        });
        let uns = arb_alloy_consensus::ArbTxEnvelope::Unsigned(ArbUnsignedTx {
            chain_id: U256::from(42161u64),
            from: address!("0000000000000000000000000000000000000003"),
            nonce: 7,
            gas_fee_cap: U256::from(1000u64),
            gas: 21000,
            to: None,
            value: U256::from(0u64),
            data: Vec::new().into(),
        });
        let enc_dep = dep.encode_typed();
        let enc_uns = uns.encode_typed();
        let payload = reth_arbitrum_payload::ArbExecutionData {
            payload: reth_arbitrum_payload::ArbPayload::new(reth_arbitrum_payload::ArbPayloadV1 {
                transactions: vec![Bytes::from(enc_dep).into(), Bytes::from(enc_uns).into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let cfg = ArbEvmConfig::<crate::tests::test_helpers::TestChainSpec, crate::tests::test_helpers::TestPrims, ArbRethReceiptBuilder>::default();
        let got: Vec<_> = cfg.arb_tx_iterator_for_payload(&payload)
            .map(|r| r.expect("ok"))
            .map(|(_enc, s)| s.tx_type())
            .collect();
        assert_eq!(got, vec![
            reth_arbitrum_primitives::ArbTxType::Deposit,
            reth_arbitrum_primitives::ArbTxType::Unsigned,
        ]);
    }
    #[test]
    fn maps_all_envelope_variants_to_tx_types() {
        use arb_alloy_consensus::tx::{
            ArbDepositTx, ArbUnsignedTx, ArbContractTx, ArbRetryTx, ArbSubmitRetryableTx, ArbInternalTx,
        };
        use alloy_primitives::{address, b256, U256};

        let dep = arb_alloy_consensus::ArbTxEnvelope::Deposit(ArbDepositTx {
            chain_id: U256::from(42161u64),
            l1_request_id: b256!("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            from: address!("00000000000000000000000000000000000000aa"),
            to: address!("00000000000000000000000000000000000000bb"),
            value: U256::from(1u64),
        });
        let uns = arb_alloy_consensus::ArbTxEnvelope::Unsigned(ArbUnsignedTx {
            chain_id: U256::from(42161u64),
            from: address!("0000000000000000000000000000000000000003"),
            nonce: 7,
            gas_fee_cap: U256::from(1000u64),
            gas: 21000,
            to: None,
            value: U256::ZERO,
            data: Vec::new().into(),
        });
        let con = arb_alloy_consensus::ArbTxEnvelope::Contract(ArbContractTx {
            chain_id: U256::from(42161u64),
            request_id: b256!("1111111111111111111111111111111111111111111111111111111111111111"),
            from: address!("0000000000000000000000000000000000000003"),
            gas_fee_cap: U256::from(1000u64),
            gas: 21000,
            to: Some(address!("0000000000000000000000000000000000000004")),
            value: U256::from(0u64),
            data: Vec::new().into(),
        });
        let rty = arb_alloy_consensus::ArbTxEnvelope::Retry(ArbRetryTx {
            chain_id: U256::from(42161u64),
            nonce: 2,
            from: address!("0000000000000000000000000000000000000005"),
            gas_fee_cap: U256::from(1000u64),
            gas: 21000,
            to: Some(address!("0000000000000000000000000000000000000006")),
            value: U256::from(0u64),
            data: Vec::new().into(),
            ticket_id: b256!("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
            refund_to: address!("0000000000000000000000000000000000000007"),
            max_refund: U256::from(0u64),
            submission_fee_refund: U256::from(0u64),
        });
        let srt = arb_alloy_consensus::ArbTxEnvelope::SubmitRetryable(ArbSubmitRetryableTx {
            chain_id: U256::from(42161u64),
            request_id: b256!("cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"),
            from: address!("0000000000000000000000000000000000000006"),
            l1_base_fee: U256::from(1u64),
            deposit_value: U256::from(2u64),
            gas_fee_cap: U256::from(4u64),
            gas: 21000,
            retry_to: Some(address!("000000000000000000000000000000000000000a")),
            retry_value: U256::from(3u64),
            beneficiary: address!("0000000000000000000000000000000000000009"),
            max_submission_fee: U256::from(5u64),
            fee_refund_addr: address!("0000000000000000000000000000000000000008"),
            retry_data: Vec::new().into(),
        });
        let itx = arb_alloy_consensus::ArbTxEnvelope::Internal(ArbInternalTx {
            chain_id: U256::from(42161u64),
            data: Vec::new().into(),
        });
        let leg = arb_alloy_consensus::ArbTxEnvelope::Legacy(Vec::new());
        fn map(env: &arb_alloy_consensus::ArbTxEnvelope) -> reth_arbitrum_primitives::ArbTxType {
            ArbEvmConfig::<crate::tests::test_helpers::TestChainSpec, crate::tests::test_helpers::TestPrims, ArbRethReceiptBuilder>::map_env_to_tx_type(env)
        }

        assert_eq!(map(&dep), reth_arbitrum_primitives::ArbTxType::Deposit);
        assert_eq!(map(&uns), reth_arbitrum_primitives::ArbTxType::Unsigned);
        assert_eq!(map(&con), reth_arbitrum_primitives::ArbTxType::Contract);
        assert_eq!(map(&rty), reth_arbitrum_primitives::ArbTxType::Retry);
        assert_eq!(map(&srt), reth_arbitrum_primitives::ArbTxType::SubmitRetryable);
        assert_eq!(map(&itx), reth_arbitrum_primitives::ArbTxType::Internal);
        assert_eq!(map(&leg), reth_arbitrum_primitives::ArbTxType::Legacy);
    }
    #[test]
    fn arb_tx_iterator_recovers_signer_for_non_legacy() {
        use arb_alloy_consensus::tx::ArbUnsignedTx;
        use alloy_primitives::{address, Bytes, U256};
        let env = arb_alloy_consensus::ArbTxEnvelope::Unsigned(ArbUnsignedTx {
            chain_id: U256::from(42161u64),
            from: address!("00000000000000000000000000000000000000aa"),
            nonce: 7,
            gas_fee_cap: U256::from(1000u64),
            gas: 21000,
            to: None,
            value: U256::ZERO,
            data: Vec::new().into(),
        });
        let enc = env.encode_typed();
        let payload = reth_arbitrum_payload::ArbExecutionData {
            payload: reth_arbitrum_payload::ArbPayload::new(reth_arbitrum_payload::ArbPayloadV1 {
                transactions: vec![Bytes::from(enc).into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let cfg = ArbEvmConfig::<crate::tests::test_helpers::TestChainSpec, crate::tests::test_helpers::TestPrims, ArbRethReceiptBuilder>::default();
        let mut it = cfg.tx_iterator_for_payload(&payload);
        let item = it.next().expect("one").expect("ok");
        let signed = item;
        let signer = *signed.signer();
        assert_eq!(signer, address!("00000000000000000000000000000000000000aa"));
    }
    #[test]
    fn arb_tx_iterator_recovers_signer_for_contract() {
        use arb_alloy_consensus::tx::ArbContractTx;
        use alloy_primitives::{address, Bytes, U256, B256};

        let env = arb_alloy_consensus::ArbTxEnvelope::Contract(ArbContractTx {
            chain_id: U256::from(42161u64),
            request_id: B256::from([0x22u8; 32]),
            from: address!("00000000000000000000000000000000000000ab"),
            gas_fee_cap: U256::from(1000u64),
            gas: 21000,
            to: None,
            value: U256::ZERO,
            data: Vec::new().into(),
        });
        let enc = env.encode_typed();
        let payload = reth_arbitrum_payload::ArbExecutionData {
            payload: reth_arbitrum_payload::ArbPayload::new(reth_arbitrum_payload::ArbPayloadV1 {
                transactions: vec![Bytes::from(enc).into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let cfg = ArbEvmConfig::<crate::tests::test_helpers::TestChainSpec, crate::tests::test_helpers::TestPrims, ArbRethReceiptBuilder>::default();
        let mut it = cfg.tx_iterator_for_payload(&payload);
        let item = it.next().expect("one").expect("ok");
        let signed = item;
        let signer = *signed.signer();
        assert_eq!(signer, address!("00000000000000000000000000000000000000ab"));
    }

    #[test]
    fn arb_tx_iterator_recovers_signer_for_retry() {
        use arb_alloy_consensus::tx::ArbRetryTx;
        use alloy_primitives::{address, Bytes, U256, B256};

        let env = arb_alloy_consensus::ArbTxEnvelope::Retry(ArbRetryTx {
            chain_id: U256::from(42161u64),
            nonce: 1,
            from: address!("00000000000000000000000000000000000000ac"),
            gas_fee_cap: U256::from(1000u64),
            gas: 21000,
            to: None,
            value: U256::ZERO,
            data: Vec::new().into(),
            ticket_id: B256::from([0x33u8; 32]),
            refund_to: address!("00000000000000000000000000000000000000ff"),
            max_refund: U256::ZERO,
            submission_fee_refund: U256::ZERO,
        });
        let enc = env.encode_typed();
        let payload = reth_arbitrum_payload::ArbExecutionData {
            payload: reth_arbitrum_payload::ArbPayload::new(reth_arbitrum_payload::ArbPayloadV1 {
                transactions: vec![Bytes::from(enc).into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let cfg = ArbEvmConfig::<crate::tests::test_helpers::TestChainSpec, crate::tests::test_helpers::TestPrims, ArbRethReceiptBuilder>::default();
        let mut it = cfg.tx_iterator_for_payload(&payload);
        let item = it.next().expect("one").expect("ok");
        let signed = item;
        let signer = *signed.signer();
        assert_eq!(signer, address!("00000000000000000000000000000000000000ac"));
    }

    #[test]
    fn arb_tx_iterator_recovers_signer_for_submit_retryable() {
        use arb_alloy_consensus::tx::ArbSubmitRetryableTx;
        use alloy_primitives::{address, Bytes, U256, B256};

        let env = arb_alloy_consensus::ArbTxEnvelope::SubmitRetryable(ArbSubmitRetryableTx {
            chain_id: U256::from(42161u64),
            request_id: B256::from([0x44u8; 32]),
            from: address!("00000000000000000000000000000000000000ad"),
            l1_base_fee: U256::from(1u64),
            deposit_value: U256::from(2u64),
            gas_fee_cap: U256::from(3u64),
            gas: 21000,
            retry_to: None,
            retry_value: U256::ZERO,
            beneficiary: address!("00000000000000000000000000000000000000ee"),
            max_submission_fee: U256::from(4u64),
            fee_refund_addr: address!("00000000000000000000000000000000000000dd"),
            retry_data: Vec::new().into(),
        });
        let enc = env.encode_typed();
        let payload = reth_arbitrum_payload::ArbExecutionData {
            payload: reth_arbitrum_payload::ArbPayload::new(reth_arbitrum_payload::ArbPayloadV1 {
                transactions: vec![Bytes::from(enc).into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let cfg = ArbEvmConfig::<crate::tests::test_helpers::TestChainSpec, crate::tests::test_helpers::TestPrims, ArbRethReceiptBuilder>::default();
        let mut it = cfg.tx_iterator_for_payload(&payload);
        let item = it.next().expect("one").expect("ok");
        let signed = item;
        let signer = *signed.signer();
        assert_eq!(signer, address!("00000000000000000000000000000000000000ad"));
    }


}

#[cfg(test)]
mod env_tests {
    use super::*;
    use alloy_consensus::Header;
use alloy_consensus::{BlockHeader as _, Transaction as _};
    use alloy_primitives::{address, b256, Bytes, B256, U256};

    #[test]
    fn evm_env_for_payload_maps_all_fields() {
        let cfg = ArbEvmConfig::<crate::tests::test_helpers::TestChainSpec, crate::tests::test_helpers::TestPrims, ArbRethReceiptBuilder>::default();

        let fee_recipient = address!("00000000000000000000000000000000000000fe");
        let prev_randao = b256!("1111111111111111111111111111111111111111111111111111111111111111");
        let base_fee = U256::from(1_234_567u64);
        let gas_limit = 30_000_000u64;
        let number = 12_345u64;
        let ts = 1_700_000_000u64;

        let payload = reth_arbitrum_payload::ArbExecutionData {
            payload: reth_arbitrum_payload::ArbPayload::new(reth_arbitrum_payload::ArbPayloadV1 {
                fee_recipient,
                prev_randao,
                gas_limit,
                base_fee_per_gas: base_fee,
                extra_data: Bytes::default().into(),
                block_number: number,
                timestamp: ts,
                gas_used: 0,
                transactions: Vec::new(),
                withdrawals: None,
                state_root: B256::ZERO,
                receipts_root: B256::ZERO,
                logs_bloom: alloy_primitives::Bloom::default(),
            }),
            sidecar: reth_arbitrum_payload::ArbSidecar { parent_beacon_block_root: Some(B256::ZERO) },
            parent_hash: B256::ZERO, block_hash: B256::ZERO,
        };

        let env = cfg.evm_env_for_payload(&payload);
        assert_eq!(env.block_env.number, U256::from(number));
        assert_eq!(env.block_env.beneficiary, fee_recipient);
        assert_eq!(env.block_env.timestamp, U256::from(ts));
        assert_eq!(env.block_env.prevrandao, Some(prev_randao));
        assert_eq!(env.block_env.gas_limit, gas_limit);
        assert_eq!(base_fee, env.block_env.basefee);
    }

    #[test]
    fn evm_env_from_header_maps_all_fields() {
        let cfg = ArbEvmConfig::<crate::tests::test_helpers::TestChainSpec, crate::tests::test_helpers::TestPrims, ArbRethReceiptBuilder>::default();

        let mut h = Header::default();
        h.number = 99;
        h.timestamp = 1_800_000_000;
        h.beneficiary = address!("00000000000000000000000000000000000000aa");
        h.mix_hash = b256!("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        h.gas_limit = 20_000_000;
        h.base_fee_per_gas = Some(42u64);
        h.difficulty = U256::from(0u64);

        let env = cfg.evm_env(&h);
        assert_eq!(env.block_env.number, U256::from(99));
        assert_eq!(env.block_env.timestamp, U256::from(1_800_000_000u64));
        assert_eq!(env.block_env.beneficiary, h.beneficiary);
        assert_eq!(env.block_env.prevrandao, Some(h.mix_hash));
        assert_eq!(env.block_env.gas_limit, 20_000_000u64);
        assert_eq!(U256::from(42u64), env.block_env.basefee);
    }
}
