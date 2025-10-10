use reth_chain_state::{ExecutedBlock, ExecutedBlockWithTrieUpdates, ExecutedTrieUpdates};
use reth_provider::DatabaseProviderFactory;
use reth_provider::DBProvider;
use reth_storage_api::{
    BlockWriter, StateWriter, TrieWriter, HistoryWriter, StageCheckpointWriter, BlockExecutionWriter,
};
use reth_provider::OriginalValuesKnown;
use reth_provider::providers::ProviderFactory;
use reth_provider::StaticFileProviderFactory;
use reth_provider::StaticFileWriter;


use alloy_consensus::BlockHeader;
use alloy_consensus::Transaction;
use alloy_consensus::transaction::TxHashRef;

use alloy_primitives::Sealable;
use reth_arbitrum_rpc::ArbNitroApiServer;
use reth_arbitrum_rpc::ArbNitroRpc;

use reth_primitives_traits::Block as _;
use reth_storage_api::BlockReader;
use reth_primitives_traits::SignedTransaction;
use super::args::RollupArgs;
use crate::follower::DynFollowerExecutor;
use reth_node_builder::rpc::EngineValidatorAddOn;

#[derive(Debug, Clone, Default)]
pub struct ArbNode {
    pub args: RollupArgs,
}

impl ArbNode {
    pub fn new(rollup_args: RollupArgs) -> Self {
        Self { args: rollup_args }
    }
}
use std::sync::Arc;

use reth_arbitrum_storage::ArbStorage;
use reth_chainspec::ChainSpec;
use reth_node_api::{FullNodeComponents, NodeTypes, PayloadAttributesBuilder, PayloadTypes};
use reth_node_builder::{
    components::{
        BasicPayloadServiceBuilder, ComponentsBuilder, ConsensusBuilder, ExecutorBuilder,
        NetworkBuilder, NoopPayloadBuilder, PoolBuilder,
    },
    node::{FullNodeTypes, NodeTypes as _},
    rpc::{
        BasicEngineValidatorBuilder, EngineApiBuilder, EngineValidatorBuilder, EthApiBuilder,
        Identity, RethRpcAddOns, RethRpcMiddleware, RethRpcServerHandles, RpcAddOns, RpcContext,
        RpcHandle, RpcHooks,
    },
    BuilderContext, DebugNode as _, NodeAdapter,
};
use reth_node_builder::{DebugNode, Node};
use reth_provider::providers::ProviderFactoryBuilder;
use reth_rpc_api::servers::DebugApiServer;
use reth_rpc_server_types::RethRpcModule;

use crate::engine::ArbEngineTypes;
use crate::follower::FollowerExecutor;
use crate::rpc::ArbEngineApiBuilder;
use reth_arbitrum_evm::{ArbEvmConfig, ArbRethReceiptBuilder};
use reth_arbitrum_payload::ArbExecutionData;
use reth_evm::ConfigureEvm;
use reth_payload_primitives::BuildNextEnv;
use reth_provider::ChainSpecProvider;
use reth_provider::{HeaderProvider, StateProviderFactory};

use alloy_eips::eip2718::Decodable2718;
impl NodeTypes for ArbNode {
    type Primitives = reth_arbitrum_primitives::ArbPrimitives;
    type ChainSpec = ChainSpec;
    type Storage = ArbStorage;
    type Payload = crate::engine::ArbEngineTypes<reth_arbitrum_payload::ArbPayloadTypes>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ArbExecutorBuilder;

impl<Types, N> ExecutorBuilder<N> for ArbExecutorBuilder
where
    Types: NodeTypes<ChainSpec = ChainSpec, Primitives = reth_arbitrum_primitives::ArbPrimitives>,
    N: FullNodeTypes<Types = Types>,
{
    type EVM = ArbEvmConfig<ChainSpec, Types::Primitives>;

    async fn build_evm(self, ctx: &BuilderContext<N>) -> eyre::Result<Self::EVM> {
        let evm_config =
            ArbEvmConfig::new(ctx.config().chain.clone(), ArbRethReceiptBuilder::default());
        Ok(evm_config)
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ArbPoolBuilder;

impl<Types, N> PoolBuilder<N> for ArbPoolBuilder
where
    Types: NodeTypes<ChainSpec = ChainSpec, Primitives = reth_arbitrum_primitives::ArbPrimitives>,
    N: FullNodeTypes<Types = Types>,
{
    type Pool = reth_transaction_pool::noop::NoopTransactionPool<
        reth_arbitrum_txpool::ArbPooledTransaction,
    >;

    async fn build_pool(self, _ctx: &BuilderContext<N>) -> eyre::Result<Self::Pool> {
        Ok(reth_transaction_pool::noop::NoopTransactionPool::new())
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ArbNetworkBuilder;

impl<N, Pool> NetworkBuilder<N, Pool> for ArbNetworkBuilder
where
    N: FullNodeTypes<Types: NodeTypes<ChainSpec = ChainSpec>>,
    Pool: reth_transaction_pool::TransactionPool<
            Transaction: reth_transaction_pool::PoolTransaction<
                Consensus = reth_node_api::TxTy<N::Types>,
            >,
        > + Unpin
        + 'static,
{
    type Network = reth_network::NetworkHandle<
        reth_network::primitives::BasicNetworkPrimitives<
            reth_node_api::PrimitivesTy<N::Types>,
            reth_transaction_pool::PoolPooledTx<Pool>,
        >,
    >;

    async fn build_network(
        self,
        ctx: &BuilderContext<N>,
        pool: Pool,
    ) -> eyre::Result<Self::Network> {
        let network = ctx.network_builder().await?;
        let handle = ctx.start_network(network, pool);
        Ok(handle)
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ArbConsensusBuilder;

impl<N> ConsensusBuilder<N> for ArbConsensusBuilder
where
    N: FullNodeTypes<
        Types: NodeTypes<
            ChainSpec = ChainSpec,
            Primitives = reth_arbitrum_primitives::ArbPrimitives,
        >,
    >,
{
    type Consensus = std::sync::Arc<crate::consensus::ArbBeaconConsensus<ChainSpec>>;

    async fn build_consensus(self, ctx: &BuilderContext<N>) -> eyre::Result<Self::Consensus> {
        Ok(std::sync::Arc::new(crate::consensus::ArbBeaconConsensus::new(ctx.chain_spec())))
    }
}

pub type ArbNodeComponents<N> = ComponentsBuilder<
    N,
    ArbPoolBuilder,
    crate::conditional_payload::ConditionalPayloadServiceBuilder<
        BasicPayloadServiceBuilder<crate::payload::ArbPayloadBuilderBuilder>,
    >,
    ArbNetworkBuilder,
    ArbExecutorBuilder,
    ArbConsensusBuilder,
>;

#[derive(Clone)]
pub struct ArbFollowerExec<N: FullNodeComponents> {
    pub provider: N::Provider,
    pub db_factory: reth_provider::providers::ProviderFactory<
        reth_node_api::NodeTypesWithDBAdapter<
            <N as reth_node_api::FullNodeTypes>::Types,
            <N as reth_node_api::FullNodeTypes>::DB
        >,
    >,
    pub beacon: reth_node_api::ConsensusEngineHandle<<N::Types as NodeTypes>::Payload>,
    pub evm_config:
        reth_arbitrum_evm::ArbEvmConfig<ChainSpec, reth_arbitrum_primitives::ArbPrimitives>,
}
impl<N> ArbFollowerExec<N>
where
    N: FullNodeComponents<
        Types: NodeTypes<
            ChainSpec = ChainSpec,
            Primitives = reth_arbitrum_primitives::ArbPrimitives,
        >,
    > + Send
        + Sync
        + 'static,
    reth_node_api::NodeTypesWithDBAdapter<
        <N as reth_node_api::FullNodeTypes>::Types,
        <N as reth_node_api::FullNodeTypes>::DB
    >: reth_provider::providers::ProviderNodeTypes
        + reth_node_api::NodeTypes<
            Primitives = reth_arbitrum_primitives::ArbPrimitives
        >,
    <<N as reth_node_api::FullNodeTypes>::Types as reth_node_api::NodeTypes>::Payload: reth_payload_primitives::PayloadTypes<
        ExecutionData = reth_arbitrum_payload::ArbExecutionData
    >,
{
    fn execute_message_to_block_sync(
        provider: &N::Provider,
        db_factory: &reth_provider::providers::ProviderFactory<
            reth_node_api::NodeTypesWithDBAdapter<
                <N as reth_node_api::FullNodeTypes>::Types,
                <N as reth_node_api::FullNodeTypes>::DB
            >
        >,
        evm_config: &reth_arbitrum_evm::ArbEvmConfig<
            ChainSpec,
            reth_arbitrum_primitives::ArbPrimitives,
        >,
        parent_hash: alloy_primitives::B256,
        attrs: alloy_rpc_types_engine::PayloadAttributes,
        l2msg_bytes: &[u8],
        poster: alloy_primitives::Address,
        request_id: Option<alloy_primitives::B256>,
        kind: u8,
        l1_block_number: u64,
        delayed_messages_read: u64,
        l1_base_fee: alloy_primitives::U256,
        batch_gas_cost: Option<u64>,
    ) -> eyre::Result<(
        alloy_primitives::B256,
        alloy_primitives::B256,
        reth_primitives_traits::SealedBlock<
            alloy_consensus::Block<reth_arbitrum_primitives::ArbTransactionSigned>
        >,
    )> {
        use reth_evm::execute::BlockBuilder;
        use reth_revm::{database::StateProviderDatabase, db::State};

        let sealed_parent = {
            let mut attempts = 0u32;
            loop {
                match provider.sealed_header_by_hash(parent_hash)? {
                    Some(h) => break h,
                    None => {
                        if attempts < 60 {
                            attempts += 1;
                            reth_tracing::tracing::debug!(target: "arb-reth::follower", want_parent=%parent_hash, attempts, "parent header not yet available; waiting");
                            std::thread::sleep(std::time::Duration::from_millis(250));
                            continue;
                        }
                        reth_tracing::tracing::error!(target: "arb-reth::follower", want_parent=%parent_hash, "missing parent header after retries");
                        return Err(eyre::eyre!("missing parent header"));
                    }
                }
            }
        };
        reth_tracing::tracing::info!(target: "arb-reth::follower", parent=%parent_hash, parent_gas_limit = sealed_parent.gas_limit(), "follower: loaded parent header");

        let l2_owned: Vec<u8> = l2msg_bytes.to_vec();

        let state_provider = provider.state_by_block_hash(parent_hash)?;
        let mut db = State::builder()
            .with_database(StateProviderDatabase::new(&state_provider))
            .with_bundle_update()
            .build();

        let mut attrs2 = attrs.clone();
        attrs2.prev_randao = sealed_parent.mix_hash().unwrap_or_default();
        reth_tracing::tracing::info!(
            target: "arb-reth::follower",
            suggested_fee_recipient = %attrs2.suggested_fee_recipient,
            poster = %poster,
            "follower: attrs2 suggested_fee_recipient before build_next_env"
        );
        let mut next_env = <reth_arbitrum_evm::ArbEvmConfig<
            ChainSpec,
            reth_arbitrum_primitives::ArbPrimitives,
        > as reth_evm::ConfigureEvm>::NextBlockEnvCtx::build_next_env(
            &reth_payload_builder::EthPayloadBuilderAttributes::new(
                parent_hash,
                attrs2.into(),
            ),
            &sealed_parent,
            evm_config.chain_spec().as_ref(),
        )
        .map_err(|e| eyre::eyre!("build_next_env error: {e}"))?;
        reth_tracing::tracing::info!(
            target: "arb-reth::follower",
            suggested_fee_recipient = %next_env.suggested_fee_recipient,
            "follower: next_env suggested_fee_recipient after build_next_env"
        );
        next_env.suggested_fee_recipient = poster;
        next_env.delayed_messages_read = delayed_messages_read;
        next_env.l1_block_number = l1_block_number;


        let chain_id = evm_config.chain_spec().chain().id();
        if chain_id == 421_614 {
            const NITRO_BASE_FEE: u64 = 0x05f5e100;
            next_env.max_fee_per_gas = Some(alloy_primitives::U256::from(NITRO_BASE_FEE));
        } else if let Some(bf) = reth_arbitrum_evm::header::read_l2_base_fee(&state_provider) {
            reth_tracing::tracing::info!(target: "arb-reth::follower", l2_base_fee = bf, "using ArbOS L2 base fee for next block");
            next_env.max_fee_per_gas = Some(alloy_primitives::U256::from(bf));
        } else {
            let parent_bf = sealed_parent.base_fee_per_gas().unwrap_or(0);
            if parent_bf > 0 {
                let parent_gas_limit = sealed_parent.gas_limit();
                let parent_gas_used = sealed_parent.gas_used();
                let target = parent_gas_limit / 8;
                let mut next_bf = parent_bf;
                if parent_gas_used > target {
                    let delta = ((parent_bf as u128)
                        * ((parent_gas_used - target) as u128)
                        / (target as u128)
                        / 8) as u64;
                    let change = if delta == 0 { 1 } else { delta };
                    next_bf = parent_bf.saturating_add(change);
                } else if parent_gas_used < target {
                    let delta = ((parent_bf as u128)
                        * ((target - parent_gas_used) as u128)
                        / (target as u128)
                        / 8) as u64;
                    let change = if delta == 0 { 1 } else { delta };
                    next_bf = parent_bf.saturating_sub(change);
                }
                reth_tracing::tracing::info!(target: "arb-reth::follower", parent_base_fee = parent_bf, parent_gas_used = parent_gas_used, parent_gas_limit = parent_gas_limit, next_base_fee = next_bf, "computed next base fee via L2 EIP-1559 (fallback)");
                next_env.max_fee_per_gas = Some(alloy_primitives::U256::from(next_bf));
            } else {
                reth_tracing::tracing::warn!(target: "arb-reth::follower", l1_base_fee = %l1_base_fee, "L2 base fee unavailable; falling back to L1 base fee");
                next_env.max_fee_per_gas = Some(l1_base_fee);
            }
        }
        let block_base_fee = next_env.max_fee_per_gas;

        const INITIAL_PER_BLOCK_GAS_LIMIT_NITRO: u64 = 0x4000000000000;
        if chain_id == 421_614 {
            next_env.gas_limit = INITIAL_PER_BLOCK_GAS_LIMIT_NITRO;
        } else if let Some(gl) = reth_arbitrum_evm::header::read_l2_per_block_gas_limit(&state_provider) {
            reth_tracing::tracing::info!(target: "arb-reth::follower", derived_gas_limit = gl, "using ArbOS per-block gas limit for next block");
            next_env.gas_limit = gl;
        } else {
            reth_tracing::tracing::warn!(target: "arb-reth::follower", "failed to read L2_PER_BLOCK_GAS_LIMIT; using Nitro default {}", INITIAL_PER_BLOCK_GAS_LIMIT_NITRO);
            next_env.gas_limit = INITIAL_PER_BLOCK_GAS_LIMIT_NITRO;
        }

        let next_block_number = sealed_parent.number() + 1;
        reth_tracing::tracing::info!(target: "arb-reth::follower", poster = %poster, next_block_number, "follower: keeping suggested_fee_recipient from attrs");

        let mut builder = evm_config
            .builder_for_next_block(&mut db, &sealed_parent, next_env)
            .map_err(|e| eyre::eyre!("builder_for_next_block error: {e}"))?;

        builder
            .apply_pre_execution_changes()
            .map_err(|e| eyre::eyre!("apply_pre_execution_changes: {e}"))?;

        fn read_u256_be32(cur: &mut &[u8]) -> eyre::Result<alloy_primitives::U256> {
            if cur.len() < 32 {
                return Err(eyre::eyre!("insufficient bytes for U256"));
            }
            let mut buf = [0u8; 32];
            buf.copy_from_slice(&cur[..32]);
            *cur = &cur[32..];
            Ok(alloy_primitives::U256::from_be_bytes(buf))
        }
        fn read_address_from_256(
            cur: &mut &[u8],
        ) -> eyre::Result<alloy_primitives::Address> {
            if cur.len() < 32 {
                return Err(eyre::eyre!("insufficient bytes for Address256"));
            }
            let mut out = [0u8; 20];
            out.copy_from_slice(&cur[12..32]);
            *cur = &cur[32..];
            Ok(alloy_primitives::Address::from(out))
        }
        fn read_address20(cur: &mut &[u8]) -> eyre::Result<alloy_primitives::Address> {
            if cur.len() < 20 {
                return Err(eyre::eyre!("insufficient bytes for Address20"));
            }
            let mut out = [0u8; 20];
            out.copy_from_slice(&cur[..20]);
            *cur = &cur[20..];
            Ok(alloy_primitives::Address::from(out))
        }
        fn read_u64_be(cur: &mut &[u8]) -> eyre::Result<u64> {
            if cur.len() < 8 {
                return Err(eyre::eyre!("insufficient bytes for u64"));
            }
            let mut buf = [0u8; 8];
            buf.copy_from_slice(&cur[..8]);
            *cur = &cur[8..];
            Ok(u64::from_be_bytes(buf))
        }
        fn u256_to_u64_checked(
            v: &alloy_primitives::U256,
            what: &str,
        ) -> eyre::Result<u64> {
            <u64 as core::convert::TryFrom<alloy_primitives::U256>>::try_from(*v)
                .map_err(|_| eyre::eyre!("{what} >= 2^64"))
        }
        fn parse_l2_message_to_txs(
            mut bytes: &[u8],
            chain_id: alloy_primitives::U256,
            poster: alloy_primitives::Address,
            request_id: Option<alloy_primitives::B256>,
        ) -> eyre::Result<Vec<reth_arbitrum_primitives::ArbTransactionSigned>>
        {
            use alloy_rlp::Decodable;
            let mut out = Vec::new();
            if bytes.is_empty() {
                return Ok(out);
            }
            let l2_kind = bytes[0];
            let mut cur = &bytes[1..];
            reth_tracing::tracing::info!(
                target: "arb-reth::decode",
                l2_kind = l2_kind,
                payload_len = cur.len(),
                "parse_l2_message_to_txs: entering with l2_kind and payload size"
            );
            match l2_kind {
                0x00 | 0x01 => {
                    let gas_limit = read_u256_be32(&mut cur)?;
                    let max_fee_per_gas = read_u256_be32(&mut cur)?;
                    let nonce = if l2_kind == 0x00 {
                        let n = read_u256_be32(&mut cur)?;
                        u256_to_u64_checked(&n, "unsigned user tx nonce")?
                    } else {
                        0u64
                    };
                    let to = read_address_from_256(&mut cur)?;
                    let to_opt =
                        if to == alloy_primitives::Address::ZERO { None } else { Some(to) };
                    let value = read_u256_be32(&mut cur)?;
                    let data = cur.to_vec();
                    let env = if l2_kind == 0x00 {
                        arb_alloy_consensus::tx::ArbTxEnvelope::Unsigned(
                            arb_alloy_consensus::tx::ArbUnsignedTx {
                                chain_id,
                                from: poster,
                                nonce,
                                gas_fee_cap: max_fee_per_gas,
                                gas: u256_to_u64_checked(
                                    &gas_limit,
                                    "unsigned/contract tx gas limit",
                                )?,
                                to: to_opt,
                                value,
                                data: alloy_primitives::Bytes::from(data),
                            },
                        )
                    } else {
                        let req = request_id.ok_or_else(|| {
                            eyre::eyre!("cannot issue contract tx without L1 request id")
                        })?;
                        arb_alloy_consensus::tx::ArbTxEnvelope::Contract(
                            arb_alloy_consensus::tx::ArbContractTx {
                                chain_id,
                                request_id: req,
                                from: poster,
                                gas_fee_cap: max_fee_per_gas,
                                gas: u256_to_u64_checked(
                                    &gas_limit,
                                    "unsigned/contract tx gas limit",
                                )?,
                                to: to_opt,
                                value,
                                data: alloy_primitives::Bytes::from(data),
                            },
                        )
                    };
                    let mut enc = env.encode_typed();
                    let mut s = enc.as_slice();
                    let tx =
                        reth_arbitrum_primitives::ArbTransactionSigned::decode_2718(&mut s)
                            .map_err(|_| {
                                eyre::eyre!("decode_2718 failed for unsigned/contract")
                            })?;
                    out.push(tx);
                }
                0x03 => {
                    let mut inner = cur;
                    let mut index: u128 = 0;
                    while inner.len() >= 8 {
                        let mut len_bytes = [0u8; 8];
                        len_bytes.copy_from_slice(&inner[..8]);
                        inner = &inner[8..];
                        let seg_len = u64::from_be_bytes(len_bytes) as usize;
                        if seg_len > inner.len() {
                            break;
                        }
                        let seg = &inner[..seg_len];
                        inner = &inner[seg_len..];

                        let sub_request_id = if let Some(req) = request_id {
                            let mut idx_be = [0u8; 32];
                            let mut tmp = index;
                            for i in (0..32).rev() {
                                idx_be[i] = (tmp & 0xff) as u8;
                                tmp >>= 8;
                            }
                            let mut data = [0u8; 64];
                            data[..32].copy_from_slice(&req.0);
                            data[32..].copy_from_slice(&idx_be);
                            Some(alloy_primitives::keccak256(data))
                        } else {
                            None
                        };

                        let mut seg_txs = parse_l2_message_to_txs(
                            seg, chain_id, poster, sub_request_id,
                        )?;
                        out.append(&mut seg_txs);
                        index = index.saturating_add(1);
                    }
                }
                0x04 => {
                    let mut s = cur;
                    while !s.is_empty() {
                        let before_len = s.len();
                        let tx = reth_arbitrum_primitives::ArbTransactionSigned::decode_2718(&mut s)
                            .map_err(|_| eyre::eyre!("decode_2718 failed for SignedTx"))?;
                        reth_tracing::tracing::info!(
                            target: "arb-reth::decode",
                            tx_type = ?tx.tx_type(),
                            tx_hash = %tx.tx_hash(),
                            remaining = s.len(),
                            "decoded 0x04 segment tx"
                        );

                        out.push(tx);
                        if s.len() == before_len {
                            break;
                        }
                    }
                }
                _ => {}
            }

            Ok(out)
        }
        fn abi_encode_u256(v: &alloy_primitives::U256) -> [u8; 32] {
            v.to_be_bytes::<32>()
        }
        fn abi_encode_u64(v: u64) -> [u8; 32] {
            let mut out = [0u8; 32];
            out[24..32].copy_from_slice(&v.to_be_bytes());
            out
        }
        fn abi_encode_address(addr: alloy_primitives::Address) -> [u8; 32] {
            let mut out = [0u8; 32];
            out[12..32].copy_from_slice(addr.as_slice());
            out
        }
        fn encode_start_block_data(
            l1_base_fee: alloy_primitives::U256,
            l1_block_number: u64,
            l2_block_number: u64,
            time_passed: u64,
        ) -> alloy_primitives::Bytes {
            const SIG: &str = "startBlock(uint256,uint64,uint64,uint64)";
            let selector = alloy_primitives::keccak256(SIG.as_bytes());
            let mut out = Vec::with_capacity(4 + 32 * 4);
            out.extend_from_slice(&selector.0[..4]);
            out.extend_from_slice(&abi_encode_u256(&l1_base_fee));
            out.extend_from_slice(&abi_encode_u64(l1_block_number));
            out.extend_from_slice(&abi_encode_u64(l2_block_number));
            out.extend_from_slice(&abi_encode_u64(time_passed));
            alloy_primitives::Bytes::from(out)
        }

        fn encode_batch_posting_report_data(
            batch_timestamp: alloy_primitives::U256,
            batch_poster: alloy_primitives::Address,
            batch_num: u64,
            batch_data_gas: u64,
            l1_base_fee: alloy_primitives::U256,
        ) -> alloy_primitives::Bytes {
            const SIG: &str = "batchPostingReport(uint256,address,uint64,uint64,uint256)";
            let selector = alloy_primitives::keccak256(SIG.as_bytes());
            let mut out = Vec::with_capacity(4 + 32 * 5);
            out.extend_from_slice(&selector.0[..4]);
            out.extend_from_slice(&abi_encode_u256(&batch_timestamp));
            out.extend_from_slice(&abi_encode_address(batch_poster));
            out.extend_from_slice(&abi_encode_u64(batch_num));
            out.extend_from_slice(&abi_encode_u64(batch_data_gas));
            out.extend_from_slice(&abi_encode_u256(&l1_base_fee));
            alloy_primitives::Bytes::from(out)
        }
        reth_tracing::tracing::info!(target: "arb-reth::follower", %kind, "follower: deriving txs for message kind");
        let chain_id_u256 =
            alloy_primitives::U256::from(evm_config.chain_spec().chain().id());
        let mut txs: Vec<reth_arbitrum_primitives::ArbTransactionSigned> = match kind {
            3 => {
                let first = l2_owned.first().copied().unwrap_or(0xff);
                let len = l2_owned.len();
                reth_tracing::tracing::info!(target: "arb-reth::follower", l2_payload_len = len, l2_first_byte = first, "follower: L2_MESSAGE payload summary");
                parse_l2_message_to_txs(&l2_owned, chain_id_u256, poster, request_id)
                    .map_err(|e| eyre::eyre!("parse_l2_message_to_txs error: {e}"))?
            },
            6 => Vec::new(),
            7 => {
                let req = request_id.ok_or_else(|| {
                    eyre::eyre!("cannot issue L2FundedByL1 tx without L1 request id")
                })?;
                if l2_owned.is_empty() {
                    return Err(eyre::eyre!("L2FundedByL1 message has no data"));
                }
                let inner_kind = l2_owned[0];
                let mut cur = &l2_owned[1..];
                let zero = alloy_primitives::U256::from(0);
                let one = alloy_primitives::U256::from(1);
                let deposit_request_id = alloy_primitives::keccak256(
                    [req.0.as_slice(), &zero.to_be_bytes::<32>()].concat(),
                );
                let unsigned_request_id = alloy_primitives::keccak256(
                    [req.0.as_slice(), &one.to_be_bytes::<32>()].concat(),
                );
                let mut l2_buf = Vec::with_capacity(1 + cur.len());
                l2_buf.push(inner_kind);
                l2_buf.extend_from_slice(cur);
                let mut derived = parse_l2_message_to_txs(
                    &l2_buf,
                    chain_id_u256,
                    poster,
                    Some(unsigned_request_id),
                )
                .map_err(|e| eyre::eyre!("parse L2FundedByL1 inner tx error: {e}"))?;
                if derived.is_empty() {
                    return Err(eyre::eyre!("L2FundedByL1 produced no inner tx"));
                }
                let value = {
                    let mut curv = &l2_buf[1..];
                    let _gas_limit = read_u256_be32(&mut curv)?;
                    let _max_fee = read_u256_be32(&mut curv)?;
                    if inner_kind == 0x00 {
                        let _nonce = read_u256_be32(&mut curv)?;
                    }
                    let _to = read_address_from_256(&mut curv)?;
                    let val = read_u256_be32(&mut curv)?;
                    val
                };
                let dep = arb_alloy_consensus::tx::ArbTxEnvelope::Deposit(
                    arb_alloy_consensus::tx::ArbDepositTx {
                        chain_id: chain_id_u256,
                        l1_request_id: deposit_request_id,
                        from: poster,
                        to: poster,
                        value,
                    },
                );
                let mut out = Vec::new();
                let mut dep_bytes = dep.encode_typed();
                let mut s1 = dep_bytes.as_slice();
                out.push(
                    reth_arbitrum_primitives::ArbTransactionSigned::decode_2718(&mut s1)
                        .map_err(|_| eyre::eyre!("decode deposit failed"))?,
                );
                out.append(&mut derived);
                out
            }
            8 => Vec::new(),
            9 => {
                let mut cur = &l2_owned[..];
                let retry_to = read_address_from_256(&mut cur)?;
                let retry_to_opt = if retry_to == alloy_primitives::Address::ZERO {
                    None
                } else {
                    Some(retry_to)
                };
                let callvalue = read_u256_be32(&mut cur)?;
                let deposit_value = read_u256_be32(&mut cur)?;
                let max_submission_fee = read_u256_be32(&mut cur)?;
                let fee_refund_addr = read_address_from_256(&mut cur)?;
                let callvalue_refund_addr = read_address_from_256(&mut cur)?;
                let gas_limit = read_u256_be32(&mut cur)?;
                let gas = u256_to_u64_checked(&gas_limit, "retryable gas limit")?;
                let max_fee_per_gas_submit = read_u256_be32(&mut cur)?;
                let data_len = read_u256_be32(&mut cur)?;
                let data_len_u64 = u256_to_u64_checked(&data_len, "retryable data length")?;
                if data_len_u64 as usize > cur.len() {
                    return Err(eyre::eyre!("retryable data too large"));
                }
                let retry_data = cur[..(data_len_u64 as usize)].to_vec();
                let req = request_id.ok_or_else(|| {
                    eyre::eyre!("cannot issue submit retryable without L1 request id")
                })?;
                let env = arb_alloy_consensus::tx::ArbTxEnvelope::SubmitRetryable(
                    arb_alloy_consensus::tx::ArbSubmitRetryableTx {
                        chain_id: chain_id_u256,
                        request_id: req,
                        from: poster,
                        l1_base_fee,
                        deposit_value,
                        gas_fee_cap: max_fee_per_gas_submit,
                        gas,
                        retry_to: retry_to_opt,
                        retry_value: callvalue,
                        beneficiary: callvalue_refund_addr,
                        max_submission_fee,
                        fee_refund_addr,
                        retry_data: alloy_primitives::Bytes::from(retry_data.clone()),
                    },
                );
                let mut enc = env.encode_typed();
                let mut s = enc.as_slice();
                let submit_tx = reth_arbitrum_primitives::ArbTransactionSigned::decode_2718(&mut s)
                    .map_err(|_| eyre::eyre!("decode submit-retryable failed"))?;
                let ticket_id = *submit_tx.tx_hash();

                let retry_gas_price = block_base_fee.unwrap_or(alloy_primitives::U256::ZERO);
                let max_refund = retry_gas_price
                    .saturating_mul(alloy_primitives::U256::from(gas))
                    .saturating_add(max_submission_fee);

                let retry_env = arb_alloy_consensus::tx::ArbTxEnvelope::Retry(
                    arb_alloy_consensus::tx::ArbRetryTx {
                        chain_id: chain_id_u256,
                        nonce: 0,
                        from: poster,
                        gas_fee_cap: retry_gas_price,
                        gas,
                        to: retry_to_opt,
                        value: callvalue,
                        data: alloy_primitives::Bytes::from(retry_data),
                        ticket_id,
                        refund_to: fee_refund_addr,
                        max_refund,
                        submission_fee_refund: max_submission_fee,
                    },
                );
                let mut retry_enc = retry_env.encode_typed();
                let mut rs = retry_enc.as_slice();
                let retry_tx = reth_arbitrum_primitives::ArbTransactionSigned::decode_2718(&mut rs)
                    .map_err(|_| eyre::eyre!("decode retry failed"))?;
                vec![submit_tx, retry_tx]
            }
            10 => return Err(eyre::eyre!("BatchForGasEstimation unimplemented")),
            11 => {
                Vec::new()
            }
            12 => {
                let mut cur = &l2_owned[..];
                let to = read_address20(&mut cur)?;
                let balance = read_u256_be32(&mut cur)?;
                let req = request_id.ok_or_else(|| {
                    eyre::eyre!("cannot issue deposit tx without L1 request id")
                })?;
                let env = arb_alloy_consensus::tx::ArbTxEnvelope::Deposit(
                    arb_alloy_consensus::tx::ArbDepositTx {
                        chain_id: chain_id_u256,
                        l1_request_id: req,
                        from: poster,
                        to,
                        value: balance,
                    },
                );
                let mut enc = env.encode_typed();
                let mut s = enc.as_slice();
                vec![reth_arbitrum_primitives::ArbTransactionSigned::decode_2718(&mut s)
                    .map_err(|_| eyre::eyre!("decode deposit failed"))?]
            }
            13 => {
                Vec::new()
            }
            0xff => {
                reth_tracing::tracing::info!(target: "arb-reth::follower", "follower: skipping invalid placeholder message kind=0xff");
                Vec::new()
            }
            _ => return Err(eyre::eyre!("unknown L2 message kind")),
        };

        {
            let is_first_startblock = txs.first().map(|tx| {
                use reth_primitives_traits::SignedTransaction;
                let is_internal = matches!(tx.tx_type(), reth_arbitrum_primitives::ArbTxType::Internal);
                let input = tx.input();
                const SIG: &str = "startBlock(uint256,uint64,uint64,uint64)";
                let selector = alloy_primitives::keccak256(SIG.as_bytes());
                let has_selector = input.as_ref().len() >= 4 && &input.as_ref()[0..4] == &selector.0[..4];
                is_internal && has_selector
            }).unwrap_or(false);

            if !is_first_startblock {
                let parent_number = sealed_parent.number();
                let parent_ts = sealed_parent.timestamp();
                let l2_block_number = parent_number.saturating_add(1);
                let time_passed = attrs.timestamp.saturating_sub(parent_ts);
                let start_data = encode_start_block_data(
                    l1_base_fee,
                    l1_block_number,
                    l2_block_number,
                    time_passed,
                );
                let env = arb_alloy_consensus::tx::ArbTxEnvelope::Internal(
                    arb_alloy_consensus::tx::ArbInternalTx {
                        chain_id: chain_id_u256,
                        data: start_data,
                    }
                );
                let mut enc = env.encode_typed();
                let mut s = enc.as_slice();
                let start_tx = reth_arbitrum_primitives::ArbTransactionSigned::decode_2718(&mut s)
                    .map_err(|_| eyre::eyre!("decode Internal failed for StartBlock"))?;
                txs.insert(0, start_tx);
            }
        }

        {
            use reth_primitives_traits::SignedTransaction;
            let tx_hashes: Vec<alloy_primitives::B256> = txs.iter().map(|t| *t.tx_hash()).collect();
            reth_tracing::tracing::info!(target: "arb-reth::follower", tx_hashes = ?tx_hashes, "follower: built tx hashes before execution");
        }

        reth_tracing::tracing::info!(target: "arb-reth::follower", txs_len = txs.len(), "follower: executing txs (including StartBlock)");

        reth_tracing::tracing::info!(target: "arb-reth::follower", tx_count = txs.len(), "follower: built tx list");
        use reth_primitives_traits::{Recovered, SignerRecoverable, SignedTransaction};
        for tx in &txs {
            let sender =
                tx.recover_signer().map_err(|_| eyre::eyre!("failed to recover signer"))?;
            let bal =
                state_provider.account_balance(&sender).ok().flatten().unwrap_or_default();
            let gp = tx.max_fee_per_gas();
            let txh = *tx.tx_hash();
            reth_tracing::tracing::info!(
                target: "arb-reth::follower",
                tx_type = ?tx.tx_type(),
                tx_hash = %txh,
                sender = %sender,
                sender_balance = %bal,
                max_fee_per_gas = %gp,
                "follower: executing tx"
            );
            let recovered = Recovered::new_unchecked(tx.clone(), sender);
            builder
                .execute_transaction(recovered)
                .map_err(|e| eyre::eyre!("execute_transaction error: {e}"))?;
        }
        let outcome = builder
            .finish(&state_provider)
            .map_err(|e| eyre::eyre!("finish error: {e}"))?;
        {
            use reth_primitives_traits::SignedTransaction as _;
            let mut fin_types: Vec<String> = Vec::new();
            let mut fin_hashes: Vec<String> = Vec::new();
            for t in outcome.block.body().transactions.iter() {
                fin_types.push(format!("{:?}", t.tx_type()));
                fin_hashes.push(format!("{:#x}", t.tx_hash()));
            }
            reth_tracing::tracing::info!(
                target: "arb-reth::follower",
                finalized_txs = fin_types.len(),
                finalized_tx_types = ?fin_types,
                finalized_tx_hashes = ?fin_hashes,
                "follower: finalized txs after finish()"
            );
        }
        let sealed_block0 = outcome.block.sealed_block().clone();
        let (mut header_unsealed, body_unsealed) = sealed_block0.clone().split_header_body();
        header_unsealed.nonce = alloy_primitives::B64::new(delayed_messages_read.to_be_bytes());
        type ArbBlock = alloy_consensus::Block<reth_arbitrum_primitives::ArbTransactionSigned, alloy_consensus::Header>;
        let sealed_block: reth_primitives_traits::block::SealedBlock<ArbBlock> =
            reth_primitives_traits::block::SealedBlock::seal_parts(header_unsealed, body_unsealed);

        let header = sealed_block.header();
        let new_block_hash = sealed_block.hash();
        
        let senders = outcome.block.senders().to_vec();
        let modified_block = reth_primitives_traits::block::RecoveredBlock::new_sealed(sealed_block.clone(), senders);
        
        let exec_outcome = reth_execution_types::ExecutionOutcome::new(
            db.take_bundle(),
            vec![outcome.execution_result.receipts.clone()],
            outcome.block.number(),
            Vec::new(),
        );
        let hashed_sorted = outcome.hashed_state.clone().into_sorted();
        {
            let provider_rw = db_factory.provider_rw().map_err(|e| eyre::eyre!("provider_rw error: {e}"))?;
            provider_rw
                .append_blocks_with_state(
                    vec![modified_block],
                    &exec_outcome,
                    hashed_sorted,
                )
                .map_err(|e| eyre::eyre!("append_blocks_with_state error: {e}"))?;
            provider_rw.commit().map_err(|e| eyre::eyre!("provider commit error: {e}"))?;
        }
        let header_hash_hex = format!("{:#x}", new_block_hash);
        let header_mix_hex = format!("{:#x}", header.mix_hash);
        let header_extra_hex = format!("{:#x}", alloy_primitives::B256::from_slice(&header.extra_data));

        let prev_randao_hex = format!("{:#x}", attrs.prev_randao);
        reth_tracing::tracing::info!(
            target: "arb-reth::follower",
            header_hash = %header_hash_hex,
            header_mix = %header_mix_hex,
            header_extra = %header_extra_hex,
            header_ts = header.timestamp,
            header_prev_randao = %prev_randao_hex,
            "follower: sealed header after finish"
        );
        reth_tracing::tracing::info!(
            target: "arb-reth::follower",
            header_beneficiary = %header.beneficiary,
            header_nonce = ?header.nonce,
            "follower: sealed header fields"
        );
        reth_tracing::tracing::info!(
            target: "arb-reth::follower",
            assembled_gas_limit = header.gas_limit,
            "follower: assembled block gas limit before import"
        );

        let new_send_root = reth_arbitrum_evm::header::extract_send_root_from_header_extra(
            header.extra_data.as_ref(),
        );

        Ok((new_block_hash, new_send_root, sealed_block))
    }
}


impl<N> FollowerExecutor for ArbFollowerExec<N>
where
    N: FullNodeComponents<
            Types: NodeTypes<
                ChainSpec = ChainSpec,
                Primitives = reth_arbitrum_primitives::ArbPrimitives,
            >,
        > + Send
        + Sync
        + 'static,
    reth_node_api::NodeTypesWithDBAdapter<
        <N as reth_node_api::FullNodeTypes>::Types,
        <N as reth_node_api::FullNodeTypes>::DB
    >: reth_provider::providers::ProviderNodeTypes
        + reth_node_api::NodeTypes<
            Primitives = reth_arbitrum_primitives::ArbPrimitives
        >,
    <<N as reth_node_api::FullNodeTypes>::Types as reth_node_api::NodeTypes>::Payload: reth_payload_primitives::PayloadTypes<
        ExecutionData = reth_arbitrum_payload::ArbExecutionData
    >,
{
    fn execute_message_to_block(
        &self,
        parent_hash: alloy_primitives::B256,
        attrs: alloy_rpc_types_engine::PayloadAttributes,
        l2msg_bytes: &[u8],
        poster: alloy_primitives::Address,
        request_id: Option<alloy_primitives::B256>,
        kind: u8,
        l1_block_number: u64,
        delayed_messages_read: u64,
        l1_base_fee: alloy_primitives::U256,
        batch_gas_cost: Option<u64>,
    ) -> core::pin::Pin<
        Box<
            dyn core::future::Future<
                    Output = eyre::Result<(alloy_primitives::B256, alloy_primitives::B256)>,
                > + Send + '_,
        >,
    > {
        let provider = self.provider.clone();
        let evm_config = self.evm_config.clone();
        let beacon = self.beacon.clone();
        let db_factory = self.db_factory.clone();
        let l2_owned: Vec<u8> = l2msg_bytes.to_vec();

        Box::pin(async move {
            let (new_block_hash, new_send_root, sealed_block) =
                Self::execute_message_to_block_sync(
                    &provider,
                    &db_factory,
                    &evm_config,
                    parent_hash,
                    attrs.clone(),
                    &l2_owned,
                    poster,
                    request_id,
                    kind,
                    l1_block_number,
                    delayed_messages_read,
                    l1_base_fee,
                    batch_gas_cost,
                )?;

            let hdr = sealed_block.header();
            reth_tracing::tracing::info!(
                target: "arb-reth::follower",
                number = hdr.number,
                gas_limit = hdr.gas_limit,
                base_fee = ?hdr.base_fee_per_gas,
                state_root = %hdr.state_root,
                receipts_root = %hdr.receipts_root,
                logs_bloom = %hdr.logs_bloom,
                mix_hash = %hdr.mix_hash,
                extra_len = hdr.extra_data.len(),
                txs = sealed_block.body().transactions.len(),
                %new_block_hash,
                "follower: pre-newPayload header summary"
            );

            let payload = reth_arbitrum_payload::ArbPayloadTypes::block_to_payload(sealed_block);
            let np = match beacon.new_payload(payload).await {
                Ok(res) => res,
                Err(e) => {
                    reth_tracing::tracing::error!(target: "arb-reth::follower", error=%e, %new_block_hash, "follower: newPayload RPC failed");
                    return Err(eyre::eyre!(e));
                }
            };
            if !matches!(np.status, alloy_rpc_types_engine::PayloadStatusEnum::Valid | alloy_rpc_types_engine::PayloadStatusEnum::Syncing) {
                reth_tracing::tracing::warn!(target: "arb-reth::follower", status=?np.status, %new_block_hash, "follower: newPayload not valid/syncing");
                eyre::bail!("newPayload status not valid/syncing: {:?}", np.status);
            }
            reth_tracing::tracing::info!(target: "arb-reth::follower", status=?np.status, latest_valid_hash=?np.latest_valid_hash, %new_block_hash, "follower: submitted newPayload for new head");

            let fcu_state = alloy_rpc_types_engine::ForkchoiceState {
                head_block_hash: new_block_hash,
                safe_block_hash: new_block_hash,
                finalized_block_hash: new_block_hash,
            };
            reth_tracing::tracing::info!(
                target: "arb-reth::follower",
                head = %fcu_state.head_block_hash,
                safe = %fcu_state.safe_block_hash,
                finalized = %fcu_state.finalized_block_hash,
                "follower: submitting FCU"
            );
            let fcu_resp = match beacon
                .fork_choice_updated(
                    fcu_state,
                    None,
                    reth_payload_primitives::EngineApiMessageVersion::default(),
                )
                .await {
                Ok(res) => res,
                Err(e) => {
                    reth_tracing::tracing::error!(target: "arb-reth::follower", error=%e, %new_block_hash, "follower: FCU RPC failed");
                    return Err(eyre::eyre!(e));
                }
            };
            if !matches!(fcu_resp.payload_status.status, alloy_rpc_types_engine::PayloadStatusEnum::Valid | alloy_rpc_types_engine::PayloadStatusEnum::Syncing) {
                reth_tracing::tracing::warn!(target: "arb-reth::follower", status=?fcu_resp.payload_status.status, latest_valid_hash=?fcu_resp.payload_status.latest_valid_hash, %new_block_hash, "follower: FCU not valid/syncing");
                eyre::bail!("forkchoiceUpdated status not valid/syncing: {:?}", fcu_resp.payload_status.status);
            }
            reth_tracing::tracing::info!(target: "arb-reth::follower", status = ?fcu_resp.payload_status.status, latest_valid_hash=?fcu_resp.payload_status.latest_valid_hash, %new_block_hash, "follower: updated forkchoice to new head");
            Ok((new_block_hash, new_send_root))
        })
    }
}
#[derive(Debug)]
pub struct ArbAddOns<
    N: FullNodeComponents,
    EthB: EthApiBuilder<N> = reth_arbitrum_rpc::ArbEthApiBuilder,
    PVB = (),
    EB = ArbEngineApiBuilder<PVB>,
    EVB = BasicEngineValidatorBuilder<crate::engine::ArbEngineValidatorBuilder>,
    RpcM = Identity,
> {
    pub rpc_add_ons: RpcAddOns<N, EthB, PVB, EB, EVB, RpcM>,
}

impl<N> Default for ArbAddOns<N>
where
    N: FullNodeComponents,
    reth_arbitrum_rpc::ArbEthApiBuilder: EthApiBuilder<N>,
{
    fn default() -> Self {
        Self { rpc_add_ons: RpcAddOns::default() }
    }
}

impl<N, EthB, PVB, EB, EVB, RpcM> ArbAddOns<N, EthB, PVB, EB, EVB, RpcM>
where
    N: FullNodeComponents,
    EthB: EthApiBuilder<N>,
{
    pub fn with_engine_api<T>(
        self,
        engine_api_builder: T,
    ) -> ArbAddOns<N, EthB, PVB, T, EVB, RpcM> {
        ArbAddOns { rpc_add_ons: self.rpc_add_ons.with_engine_api(engine_api_builder) }
    }

    pub fn with_payload_validator<T>(
        self,
        payload_validator_builder: T,
    ) -> ArbAddOns<N, EthB, T, EB, EVB, RpcM> {
        ArbAddOns {
            rpc_add_ons: self.rpc_add_ons.with_payload_validator(payload_validator_builder),
        }
    }

    pub fn with_rpc_middleware<T>(self, rpc_middleware: T) -> ArbAddOns<N, EthB, PVB, EB, EVB, T> {
        ArbAddOns { rpc_add_ons: self.rpc_add_ons.with_rpc_middleware(rpc_middleware) }
    }
}

impl<N, EthB, PVB, EB, EVB, RpcM> reth_node_api::NodeAddOns<N>
    for ArbAddOns<N, EthB, PVB, EB, EVB, RpcM>
where
    N: FullNodeComponents<
        Types: NodeTypes<
            ChainSpec = ChainSpec,
            Primitives = reth_arbitrum_primitives::ArbPrimitives,
        >,
    > + reth_node_api::ProviderFactoryExt,
    <<N as reth_node_api::FullNodeTypes>::Types as reth_node_api::NodeTypes>::Storage:
        reth_provider::providers::ChainStorage<reth_arbitrum_primitives::ArbPrimitives>,
    <<N as reth_node_api::FullNodeTypes>::Types as reth_node_api::NodeTypes>::Payload: reth_payload_primitives::PayloadTypes<
        ExecutionData = reth_arbitrum_payload::ArbExecutionData
    >,
    EthB: EthApiBuilder<N>,
    PVB: Send,
    EB: EngineApiBuilder<N>,
    EVB: EngineValidatorBuilder<N>,
    RpcM: RethRpcMiddleware,
{
    type Handle = RpcHandle<N, EthB::EthApi>;

    async fn launch_add_ons(
        self,
        ctx: reth_node_api::AddOnsContext<'_, N>,
    ) -> eyre::Result<Self::Handle> {
        let rpc_add_ons = self.rpc_add_ons;
        let rpc_handle = rpc_add_ons
            .launch_add_ons_with(ctx.clone(), move |container| {
                let reth_node_builder::rpc::RpcModuleContainer { modules, .. } = container;
                let arb_rpc = ArbNitroRpc::default();
                modules.merge_configured(arb_rpc.into_rpc())?;
                Ok(())
            })
            .await?;

        let follower: ArbFollowerExec<N> = ArbFollowerExec {
            provider: ctx.node.provider().clone(),
            db_factory: reth_node_api::ProviderFactoryExt::provider_factory(&ctx.node).clone(),
            beacon: rpc_handle.beacon_engine_handle.clone(),
            evm_config: reth_arbitrum_evm::ArbEvmConfig::new(
                ctx.config.chain.clone(),
                reth_arbitrum_evm::ArbRethReceiptBuilder::default(),
            ),
        };
        crate::follower::set_follower_executor(Arc::new(follower));
        Ok(rpc_handle)
    }
}

impl<N, EthB, PVB, EB, EVB, RpcM> RethRpcAddOns<N> for ArbAddOns<N, EthB, PVB, EB, EVB, RpcM>
where
    N: FullNodeComponents<
        Types: NodeTypes<
            ChainSpec = ChainSpec,
            Primitives = reth_arbitrum_primitives::ArbPrimitives,
        >,
    > + reth_node_api::ProviderFactoryExt,
    <<N as reth_node_api::FullNodeTypes>::Types as reth_node_api::NodeTypes>::Storage:
        reth_provider::providers::ChainStorage<reth_arbitrum_primitives::ArbPrimitives>,
    <<N as reth_node_api::FullNodeTypes>::Types as reth_node_api::NodeTypes>::Payload: reth_payload_primitives::PayloadTypes<
        ExecutionData = reth_arbitrum_payload::ArbExecutionData
    >,
    EthB: EthApiBuilder<N>,
    PVB: Send,
    EB: EngineApiBuilder<N>,
    EVB: EngineValidatorBuilder<N>,
    RpcM: RethRpcMiddleware,
{
    type EthApi = EthB::EthApi;

    fn hooks_mut(&mut self) -> &mut RpcHooks<N, Self::EthApi>
    where
        <<N as reth_node_api::FullNodeTypes>::Types as reth_node_api::NodeTypes>::Storage:
            reth_provider::providers::ChainStorage<reth_arbitrum_primitives::ArbPrimitives>,
    {
        &mut self.rpc_add_ons.hooks
    }
}
impl<N, EthB, PVB, EB, EVB, RpcM> reth_node_builder::rpc::EngineValidatorAddOn<N>
    for ArbAddOns<N, EthB, PVB, EB, EVB, RpcM>
where
    N: FullNodeComponents,
    EthB: EthApiBuilder<N>,
    PVB: Send,
    EB: EngineApiBuilder<N>,
    EVB: EngineValidatorBuilder<N> + Default,
    RpcM: RethRpcMiddleware,
{
    type ValidatorBuilder = EVB;

    fn engine_validator_builder(&self) -> Self::ValidatorBuilder {
        EVB::default()
    }
}

impl<N> Node<N> for ArbNode
where
    N: FullNodeTypes<Types = Self>,
{
    type ComponentsBuilder = ArbNodeComponents<N>;

    type AddOns =
        ArbAddOns<
            NodeAdapter<
                N,
                <Self::ComponentsBuilder as reth_node_builder::components::NodeComponentsBuilder<
                    N,
                >>::Components,
            >,
            reth_arbitrum_rpc::ArbEthApiBuilder,
            (),
            ArbEngineApiBuilder<crate::validator::ArbPayloadValidatorBuilder>,
            BasicEngineValidatorBuilder<crate::engine::ArbEngineValidatorBuilder>,
            Identity,
        >;

    fn components_builder(&self) -> <Self as Node<N>>::ComponentsBuilder {
        ComponentsBuilder::default()
            .node_types::<N>()
            .pool(ArbPoolBuilder::default())
            .executor(ArbExecutorBuilder::default())
            .payload(crate::conditional_payload::ConditionalPayloadServiceBuilder::new(
                BasicPayloadServiceBuilder::new(crate::payload::ArbPayloadBuilderBuilder::default()),
                self.args.sequencer,
            ))
            .network(ArbNetworkBuilder::default())
            .consensus(ArbConsensusBuilder::default())
    }

    fn add_ons(&self) -> <Self as Node<N>>::AddOns {
        let engine_api =
            ArbEngineApiBuilder::new(crate::validator::ArbPayloadValidatorBuilder::default());
        let add_ons: <Self as Node<N>>::AddOns =
            ArbAddOns::default().with_engine_api(engine_api);
        add_ons
    }
}

impl<N> DebugNode<N> for ArbNode
where
    N: FullNodeComponents<Types = Self>,
{
    type RpcBlock = alloy_rpc_types_eth::Block<reth_arbitrum_primitives::ArbTransactionSigned>;

    fn rpc_to_primitive_block(
        rpc_block: <Self as DebugNode<N>>::RpcBlock,
    ) -> reth_node_api::BlockTy<Self> {
        rpc_block.into_consensus()
    }

    fn local_payload_attributes_builder(
        chain_spec: &<Self as reth_node_api::NodeTypes>::ChainSpec,
    ) -> impl PayloadAttributesBuilder<<Self::Payload as PayloadTypes>::PayloadAttributes> {
        reth_engine_local::LocalPayloadAttributesBuilder::new(Arc::new(chain_spec.clone()))
    }
}
