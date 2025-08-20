use alloy_consensus::BlockHeader;

use alloy_consensus::Transaction;

use reth_arbitrum_rpc::ArbNitroApiServer;
use reth_arbitrum_rpc::ArbNitroRpc;

use super::args::RollupArgs;
use crate::follower::DynFollowerExecutor;

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
use reth_trie_db::MerklePatriciaTrie;

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
    type StateCommitment = MerklePatriciaTrie;
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
    pub beacon: reth_node_api::ConsensusEngineHandle<<N::Types as NodeTypes>::Payload>,
    pub evm_config:
        reth_arbitrum_evm::ArbEvmConfig<ChainSpec, reth_arbitrum_primitives::ArbPrimitives>,
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
{
    fn execute_message_to_block(
        &self,
        parent_hash: alloy_primitives::B256,
        attrs: alloy_rpc_types_engine::PayloadAttributes,
        l2msg_bytes: &[u8],
        poster: alloy_primitives::Address,
        request_id: Option<alloy_primitives::B256>,
        kind: u8,
        l1_base_fee: alloy_primitives::U256,
        batch_gas_cost: Option<u64>,
    ) -> core::pin::Pin<
        Box<
            dyn core::future::Future<
                    Output = eyre::Result<(alloy_primitives::B256, alloy_primitives::B256)>,
                > + Send,
        >,
    > {
        use alloy_rpc_types_engine::{ForkchoiceState, PayloadStatusEnum};
        use reth_evm::execute::BlockBuilder;
        use reth_payload_primitives::EngineApiMessageVersion;
        use reth_revm::{database::StateProviderDatabase, db::State};

        let provider = self.provider.clone();
        let evm_config = self.evm_config.clone();
        let beacon = self.beacon.clone();
        let l2_owned: Vec<u8> = l2msg_bytes.to_vec();

        Box::pin(async move {
            let _ = beacon
                .fork_choice_updated(
                    ForkchoiceState {
                        head_block_hash: parent_hash,
                        safe_block_hash: parent_hash,
                        finalized_block_hash: parent_hash,
                    },
                    None,
                    EngineApiMessageVersion::default(),
                )
                .await?;

            let parent_header = {
                let mut attempts = 0u32;
                loop {
                    match provider.header(&parent_hash)? {
                        Some(h) => break h,
                        None => {
                            if attempts < 60 {
                                attempts += 1;
                                reth_tracing::tracing::debug!(target: "arb-reth::follower", want_parent=%parent_hash, attempts, "parent header not yet available; waiting");
                                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                                continue;
                            }
                            let gen_opt = provider.header_by_number(0)?;
                            if let Some(gen) = gen_opt {
                                let gh = reth_primitives_traits::SealedHeader::new(
                                    gen.clone(),
                                    gen.hash_slow(),
                                )
                                .hash();
                                reth_tracing::tracing::error!(target: "arb-reth::follower", want_parent=%parent_hash, have_genesis=%gh, "missing parent header; canonical genesis differs?");
                            } else {
                                reth_tracing::tracing::error!(target: "arb-reth::follower", want_parent=%parent_hash, "missing parent header; canonical genesis not found");
                            }
                            return Err(eyre::eyre!("missing parent header"));
                        }
                    }
                }
            };
            reth_tracing::tracing::info!(target: "arb-reth::follower", parent=%parent_hash, parent_gas_limit = parent_header.gas_limit, "follower: loaded parent header");

            let sealed_parent =
                reth_primitives_traits::SealedHeader::new(parent_header, parent_hash);

            let (exec_data, block_hash, send_root) = {
                let state_provider = provider.state_by_block_hash(parent_hash)?;
                let mut db = State::builder()
                    .with_database(StateProviderDatabase::new(&state_provider))
                    .with_bundle_update()
                    .build();

                let mut next_env = <reth_arbitrum_evm::ArbEvmConfig<
                    ChainSpec,
                    reth_arbitrum_primitives::ArbPrimitives,
                > as reth_evm::ConfigureEvm>::NextBlockEnvCtx::build_next_env(
                    &reth_payload_builder::EthPayloadBuilderAttributes::new(
                        parent_hash,
                        attrs.clone().into(),
                    ),
                    &sealed_parent,
                    evm_config.chain_spec().as_ref(),
                )
                .map_err(|e| eyre::eyre!("build_next_env error: {e}"))?;

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
                    reth_tracing::tracing::info!(target: "arb-reth::follower", parent_base_fee = parent_bf, parent_gas_used = parent_gas_used, parent_gas_limit = parent_gas_limit, next_base_fee = next_bf, "computed next base fee via L2 EIP-1559");
                    next_env.max_fee_per_gas = Some(alloy_primitives::U256::from(next_bf));
                } else if let Some(bf) = reth_arbitrum_evm::header::read_l2_base_fee(&state_provider) {
                    reth_tracing::tracing::info!(target: "arb-reth::follower", l2_base_fee = bf, "using ArbOS L2 base fee for next block");
                    next_env.max_fee_per_gas = Some(alloy_primitives::U256::from(bf));
                } else {
                    reth_tracing::tracing::warn!(target: "arb-reth::follower", l1_base_fee = %l1_base_fee, "L2 base fee unavailable; falling back to L1 base fee");
                    next_env.max_fee_per_gas = Some(l1_base_fee);
                }
                let block_base_fee = next_env.max_fee_per_gas;


                if next_env.gas_limit == 0 {
                    if let Some(gl) =
                        reth_arbitrum_evm::header::read_l2_per_block_gas_limit(&state_provider)
                    {
                        reth_tracing::tracing::info!(target: "arb-reth::follower", derived_gas_limit = gl, "overriding zero gas_limit from ArbOS state");
                        next_env.gas_limit = gl;
                    } else {
                        const INITIAL_PER_BLOCK_GAS_LIMIT_V0: u64 = 20_000_000;
                        reth_tracing::tracing::warn!(target: "arb-reth::follower", "failed to read L2_PER_BLOCK_GAS_LIMIT; using default {}", INITIAL_PER_BLOCK_GAS_LIMIT_V0);
                        next_env.gas_limit = INITIAL_PER_BLOCK_GAS_LIMIT_V0;
                    }
                }

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
                            while !inner.is_empty() {
                                match <Vec<u8> as Decodable>::decode(&mut inner) {
                                    Ok(seg) => {
                                        let mut seg_txs = parse_l2_message_to_txs(
                                            &seg, chain_id, poster, request_id,
                                        )?;
                                        out.append(&mut seg_txs);
                                    }
                                    Err(_) => break,
                                }
                            }
                        }
                        0x04 => {
                            let mut s = cur;
                            let tx =
                                reth_arbitrum_primitives::ArbTransactionSigned::decode_2718(&mut s)
                                    .map_err(|_| eyre::eyre!("decode_2718 failed for SignedTx"))?;
                            out.push(tx);
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
                    3 => parse_l2_message_to_txs(&l2_owned, chain_id_u256, poster, request_id)
                        .map_err(|e| eyre::eyre!("parse_l2_message_to_txs error: {e}"))?,
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
                        let max_fee_per_gas = read_u256_be32(&mut cur)?;
                        let min_bf = block_base_fee.unwrap_or(alloy_primitives::U256::ZERO);
                        let max_fee_per_gas = if max_fee_per_gas < min_bf { min_bf } else { max_fee_per_gas };
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
                                gas_fee_cap: max_fee_per_gas,
                                gas,
                                retry_to: retry_to_opt,
                                retry_value: callvalue,
                                beneficiary: callvalue_refund_addr,
                                max_submission_fee,
                                fee_refund_addr,
                                retry_data: alloy_primitives::Bytes::from(retry_data),
                            },
                        );
                        let mut enc = env.encode_typed();
                        let mut s = enc.as_slice();
                        vec![reth_arbitrum_primitives::ArbTransactionSigned::decode_2718(&mut s)
                            .map_err(|_| eyre::eyre!("decode submit-retryable failed"))?]
                    }
                    10 => return Err(eyre::eyre!("BatchForGasEstimation unimplemented")),
                    11 => Vec::new(),
                    12 => {
                        let mut cur = &l2_owned[..];
                        let to = read_address20(&mut cur)?;
                        let value = read_u256_be32(&mut cur)?;
                        let req = request_id.ok_or_else(|| {
                            eyre::eyre!("cannot issue deposit without L1 request id")
                        })?;
                        let env = arb_alloy_consensus::tx::ArbTxEnvelope::Deposit(
                            arb_alloy_consensus::tx::ArbDepositTx {
                                chain_id: chain_id_u256,
                                l1_request_id: req,
                                from: poster,
                                to,
                                value,
                            },
                        );
                        let mut enc = env.encode_typed();
                        let mut s = enc.as_slice();
                        vec![reth_arbitrum_primitives::ArbTransactionSigned::decode_2718(&mut s)
                            .map_err(|_| eyre::eyre!("decode deposit failed"))?]
                    }
                    13 => {
                        let mut cur = &l2_owned[..];
                        let batch_timestamp = read_u256_be32(&mut cur)?;
                        let batch_poster_addr = read_address20(&mut cur)?;
                        let _data_hash = read_u256_be32(&mut cur)?;
                        let batch_num_u256 = read_u256_be32(&mut cur)?;
                        let batch_num = u256_to_u64_checked(&batch_num_u256, "batch number")?;
                        let l1bf = read_u256_be32(&mut cur)?;
                        let extra_gas = if cur.is_empty() { 0u64 } else { read_u64_be(&mut cur)? };
                        let base_data_gas = batch_gas_cost
                            .ok_or_else(|| eyre::eyre!("cannot compute batch gas cost"))?;
                        let batch_data_gas = base_data_gas.saturating_add(extra_gas);
                        let data = encode_batch_posting_report_data(
                            batch_timestamp,
                            batch_poster_addr,
                            batch_num,
                            batch_data_gas,
                            l1bf,
                        );
                        let env = arb_alloy_consensus::tx::ArbTxEnvelope::Internal(
                            arb_alloy_consensus::tx::ArbInternalTx {
                                chain_id: chain_id_u256,
                                data,
                            },
                        );
                        let mut enc = env.encode_typed();
                        let mut s = enc.as_slice();
                        vec![reth_arbitrum_primitives::ArbTransactionSigned::decode_2718(&mut s)
                            .map_err(|_| eyre::eyre!("decode internal failed"))?]
                    }
                    0xFF => return Err(eyre::eyre!("invalid message kind")),
                    other => return Err(eyre::eyre!(format!("invalid message type {other}"))),
                };
                reth_tracing::tracing::info!(target: "arb-reth::follower", tx_count = txs.len(), "follower: built tx list");
                use reth_primitives_traits::{Recovered, SignerRecoverable};
                for tx in &txs {
                    let sender =
                        tx.recover_signer().map_err(|_| eyre::eyre!("failed to recover sender"))?;
                    let bal =
                        state_provider.account_balance(&sender).ok().flatten().unwrap_or_default();
                    let gp = tx.max_fee_per_gas();
                    reth_tracing::tracing::info!(
                        target: "arb-reth::follower",
                        tx_type = ?tx.tx_type(),
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
                let sealed_block = outcome.block.sealed_block().clone();

                let header = sealed_block.header();
                reth_tracing::tracing::info!(target: "arb-reth::follower", assembled_gas_limit = header.gas_limit, "follower: assembled block gas limit before import");

                let block_hash = sealed_block.hash();
                let send_root = reth_arbitrum_evm::header::extract_send_root_from_header_extra(
                    header.extra_data.as_ref(),
                );

                let exec_data =
                    <<N as reth_node_api::FullNodeTypes>::Types as reth_node_api::NodeTypes>::Payload::block_to_payload(sealed_block);
                (exec_data, block_hash, send_root)
            };

            let res = beacon.new_payload(exec_data).await?;
            match res.status {
                PayloadStatusEnum::Valid
                | PayloadStatusEnum::Accepted
                | PayloadStatusEnum::Syncing => {}
                other => return Err(eyre::eyre!("new_payload returned status {other:?}")),
            }

            let fcu = ForkchoiceState {
                head_block_hash: block_hash,
                safe_block_hash: parent_hash,
                finalized_block_hash: parent_hash,
            };
            let fcu_res =
                beacon.fork_choice_updated(fcu, None, EngineApiMessageVersion::default()).await?;
            if !matches!(
                fcu_res.payload_status.status,
                PayloadStatusEnum::Valid | PayloadStatusEnum::Syncing
            ) {
                return Err(eyre::eyre!(
                    "fork_choice_updated not valid/syncing: {:?}",
                    fcu_res.payload_status
                ));
            }
            match provider.header(&block_hash) {
                Ok(Some(_)) => {
                    reth_tracing::tracing::info!(target: "arb-reth::follower", %block_hash, "follower: new block header visible after FCU");
                }
                Ok(None) => {
                    reth_tracing::tracing::warn!(target: "arb-reth::follower", %block_hash, "follower: new block header NOT visible after FCU");
                }
                Err(e) => {
                    reth_tracing::tracing::warn!(target: "arb-reth::follower", %block_hash, err = %e, "follower: error checking new block header");
                }
            }
            match provider.header(&parent_hash) {
                Ok(Some(_)) => {
                    reth_tracing::tracing::info!(target: "arb-reth::follower", %parent_hash, "follower: parent header visible after FCU");
                }
                Ok(None) => {
                    reth_tracing::tracing::warn!(target: "arb-reth::follower", %parent_hash, "follower: parent header NOT visible after FCU");
                }
                Err(e) => {
                    reth_tracing::tracing::warn!(target: "arb-reth::follower", %parent_hash, err = %e, "follower: error checking parent header");
                }
            }
            Ok((block_hash, send_root))
        })
    }
}
#[derive(Debug)]
pub struct ArbAddOns<
    N: FullNodeComponents,
    EthB: EthApiBuilder<N> = reth_arbitrum_rpc::ArbEthApiBuilder,
    PVB = (),
    EB = ArbEngineApiBuilder<PVB>,
    EVB = BasicEngineValidatorBuilder<crate::validator::ArbPayloadValidatorBuilder>,
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
    >,
    EthB: EthApiBuilder<N>,
    PVB: Send,
    EB: EngineApiBuilder<N>,
    EVB: EngineValidatorBuilder<N>,
    RpcM: RethRpcMiddleware,
{
    type EthApi = EthB::EthApi;

    fn hooks_mut(&mut self) -> &mut RpcHooks<N, Self::EthApi> {
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
            BasicEngineValidatorBuilder<crate::validator::ArbPayloadValidatorBuilder>,
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
        ArbAddOns::default().with_engine_api(engine_api)
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
