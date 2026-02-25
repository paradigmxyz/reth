//! Mantle Eth API extension implementation.

use crate::error::SequencerClientError;
use crate::SequencerClient;
use alloy_consensus::BlockHeader;
use alloy_eips::{BlockId, BlockNumberOrTag, Encodable2718};
use alloy_primitives::Sealable;
use alloy_network::TransactionBuilder;
use alloy_primitives::{Bytes, U256};
use jsonrpsee::types::ErrorObject;
use jsonrpsee_core::RpcResult;
use reth_chainspec::{ChainSpecProvider, EthChainSpec};
use reth_optimism_evm::RethL1BlockInfo;
use reth_rpc_eth_api::{
    helpers::Call, EthApiServer, EthApiTypes, MantleEthApiExtServer, PreconfTxEvent, RpcBlock,
    RpcNodeCore,
};
use reth_rpc_server_types::result::invalid_params_rpc_err;
use reth_storage_api::{BlockNumReader, BlockReaderIdExt, StateProviderFactory};
use reth_mantle_forks::MantleHardforks;
use op_revm::constants::{GAS_ORACLE_CONTRACT, TOKEN_RATIO_SLOT};
use reth_rpc_convert::TryIntoSimTx;
use alloy_rpc_types_eth::TransactionRequest;
use reth_optimism_evm::extract_l1_info;
use reth_primitives_traits::Block;
use std::sync::Arc;
use tracing::info;

/// Mantle-specific `Eth` API extensions implementation.
///
/// This provides Mantle-specific RPC methods such as `getBlockRange` and
/// `sendRawTransactionWithPreconf`.
#[derive(Clone, Debug)]
pub struct MantleEthApiExt<Provider, EthApi> {
    /// The provider type used to interact with the node.
    provider: Provider,
    /// The Eth API used to fetch blocks.
    eth_api: Arc<EthApi>,
    /// Optional sequencer client for forwarding transactions.
    sequencer_client: Option<SequencerClient>,
}

impl<Provider, EthApi> MantleEthApiExt<Provider, EthApi> {
    /// Creates a new [`MantleEthApiExt`].
    pub fn new(
        provider: Provider,
        eth_api: Arc<EthApi>,
        sequencer_client: Option<SequencerClient>,
    ) -> Self {
        Self { provider, eth_api, sequencer_client }
    }

    #[inline]
    fn provider(&self) -> &Provider {
        &self.provider
    }

    #[inline]
    fn eth_api(&self) -> &EthApi {
        &self.eth_api
    }
}

#[async_trait::async_trait]
impl<Provider, EthApi> MantleEthApiExtServer<RpcBlock<EthApi::NetworkTypes>>
    for MantleEthApiExt<Provider, EthApi>
where
    Provider: BlockReaderIdExt
        + BlockNumReader
        + ChainSpecProvider<ChainSpec: MantleHardforks>
        + StateProviderFactory
        + Clone
        + Send
        + Sync
        + 'static,
    reth_rpc_eth_api::RpcTxReq<EthApi::NetworkTypes>: reth_rpc_convert::TryIntoSimTx<op_alloy_consensus::OpTxEnvelope>
        + Clone,
    EthApi: Call
        + RpcNodeCore
        + EthApiTypes
        + EthApiServer<
            reth_rpc_eth_api::RpcTxReq<EthApi::NetworkTypes>,
            reth_rpc_eth_api::RpcTransaction<EthApi::NetworkTypes>,
            RpcBlock<EthApi::NetworkTypes>,
            reth_rpc_eth_api::RpcReceipt<EthApi::NetworkTypes>,
            reth_rpc_eth_api::RpcHeader<EthApi::NetworkTypes>,
            reth_primitives_traits::TxTy<EthApi::Primitives>,
        > + Send
        + Sync
        + 'static,
{
    /// Returns a range of blocks.
    ///
    /// # Deprecation
    ///
    /// This method is deprecated and will be removed in the next network upgrade.
    // #[deprecated(note = "This method will be removed in the next network upgrade.")]
    async fn get_block_range(
        &self,
        start_number: BlockNumberOrTag,
        end_number: BlockNumberOrTag,
        full_transactions: bool,
    ) -> RpcResult<Vec<RpcBlock<EthApi::NetworkTypes>>> {
        // Convert BlockNumberOrTag to actual block numbers
        let start = self
            .provider()
            .convert_block_number(start_number)
            .map_err(|e| {
                ErrorObject::owned(
                    -32000,
                    format!("Failed to convert start block number: {e}"),
                    None::<()>,
                )
            })?
            .ok_or_else(|| invalid_params_rpc_err("Start block number not found"))?;

        let end = self
            .provider()
            .convert_block_number(end_number)
            .map_err(|e| {
                ErrorObject::owned(
                    -32000,
                    format!("Failed to convert end block number: {e}"),
                    None::<()>,
                )
            })?
            .ok_or_else(|| invalid_params_rpc_err("End block number not found"))?;

        // Validate: start must be less than or equal to end
        if end < start {
            return Err(invalid_params_rpc_err(format!(
                "start of block range ({start}) is greater than end of block range ({end})"
            )));
        }

        // Validate: range cannot exceed 1000 blocks
        const MAX_BLOCK_RANGE: u64 = 1000;
        let range_size = end.saturating_sub(start).saturating_add(1);
        if range_size > MAX_BLOCK_RANGE {
            return Err(invalid_params_rpc_err(format!(
                "requested block range is too large (max is {MAX_BLOCK_RANGE}, requested {range_size} blocks)"
            )));
        }

        // Validate: end block must exist
        if self.eth_api().block_by_number(end_number, full_transactions).await?.is_none() {
            return Err(invalid_params_rpc_err(format!(
                "end of requested block range ({end}) does not exist"
            )));
        }

        // Collect all blocks in the range
        let mut blocks = Vec::new();
        for block_num in start..=end {
            let block = self
                .eth_api()
                .block_by_number(BlockNumberOrTag::Number(block_num), full_transactions)
                .await?
                .ok_or_else(|| ErrorObject::owned(
                    -32000,
                    format!("block in range not indexed, this should never happen: block {block_num}"),
                    None::<()>
                ))?;
            blocks.push(block);
        }

        Ok(blocks)
    }

    async fn send_raw_transaction_with_preconf(&self, bytes: Bytes) -> RpcResult<PreconfTxEvent> {
        // If we have a sequencer client, forward the transaction to it
        if let Some(sequencer) = self.sequencer_client.as_ref() {
            tracing::debug!(target: "rpc::eth", "forwarding raw transaction with preconf to sequencer");
            sequencer
                .forward_raw_transaction_with_preconf(bytes.as_ref())
                .await
                .map_err(|err| {
                    // Extract the original error message from the sequencer response
                    // SequencerClientError only has one variant (HttpError), so we can directly destructure
                    let SequencerClientError::HttpError(rpc_err) = &err;
                    let error_msg = rpc_err
                        .as_error_resp()
                        .map(|payload| payload.message.to_string())
                        .unwrap_or_else(|| err.to_string());

                    ErrorObject::owned(
                        -32000,
                        format!("failed to forward tx to sequencer, please try again. Error message: '{error_msg}'"),
                        None::<()>,
                    )
                })
        } else {
            // If no sequencer client is available, return an error
            Err(ErrorObject::owned(
                -32000,
                "sendRawTransactionWithPreconf is not yet implemented: sequencer client not configured",
                None::<()>,
            ))
        }
    }

    async fn estimate_total_fee(
        &self,
        request: TransactionRequest,
        block_number: Option<BlockId>,
    ) -> RpcResult<U256> {
        use alloy_eips::BlockNumberOrTag;

        let block_id = block_number.unwrap_or(BlockId::Number(BlockNumberOrTag::Latest));

        let block = self
            .provider()
            .block_by_id(block_id)
            .map_err(|e| ErrorObject::owned(-32000, format!("failed to get block: {e}"), None::<()>))?
            .ok_or_else(|| invalid_params_rpc_err("Block not found"))?;

        let header = block.header();
        let chain_spec = self.provider().chain_spec();

        if !chain_spec.is_arsia_active_at_timestamp(header.timestamp()) {
            return Err(ErrorObject::owned(
                -32000,
                "eth_estimateTotalFee is not supported for pre-Arsia blocks",
                None::<()>,
            )
            .into());
        }

        let mut req_value = serde_json::to_value(&request).map_err(|e| {
            ErrorObject::owned(-32000, format!("invalid request: {e}"), None::<()>)
        })?;
        // Inject chainId when missing or null so build_typed_tx() can encode the tx for L1 fee
        if req_value.get("chainId").map_or(true, |v| v.is_null()) {
            req_value["chainId"] =
                serde_json::Value::String(format!("0x{:x}", chain_spec.chain().id()));
        }
        let mut op_req: reth_rpc_eth_api::RpcTxReq<EthApi::NetworkTypes> =
            serde_json::from_value(req_value)
                .map_err(|e| ErrorObject::owned(-32000, format!("invalid request: {e}"), None::<()>))?;

        // Ensure chain_id is set so build_typed_tx() can encode the tx for L1 fee (JSON may omit or use wrong key)
        if op_req.as_ref().chain_id.is_none() {
            op_req
                .as_mut()
                .set_chain_id(chain_spec.chain().id());
        }

        let gas_estimate = self
            .eth_api()
            .estimate_gas(op_req.clone(), Some(block_id), None)
            .await
            .map_err(|e| {
                ErrorObject::owned(
                    -32000,
                    format!("failed to estimate gas: {}", e.message()),
                    None::<()>,
                )
            })?;

        let base_fee = U256::from(header.base_fee_per_gas().unwrap_or(0));
        let gas_price = match (request.gas_price, request.max_fee_per_gas) {
            (Some(gp), _) if gp > 0 => U256::from(gp),
            (_, Some(max_fee)) if max_fee > 0 => {
                let tip = U256::from(request.max_priority_fee_per_gas.unwrap_or(0));
                let effective = base_fee.saturating_add(tip);
                effective.min(U256::from(max_fee))
            }
            _ => {
                let tip = self
                    .eth_api()
                    .max_priority_fee_per_gas()
                    .await
                    .unwrap_or(U256::ZERO);
                base_fee.saturating_add(tip)
            }
        };

        let l2_fee = gas_estimate.saturating_mul(gas_price);

        // Align with geth: tx for L1 cost is built with CallDefaults (gas_cap when gas nil, nonce 0 when nil, fee fields 0 when nil).
        // Geth uses RPCGasCap = DefaultMantleBlockGasLimit (0x4000000000000); use same so L1-cost encoded tx matches geth.
        const GETH_MANTLE_RPC_GAS_CAP: u64 = 0x4000000000000; // op-geth core/blockchain.go DefaultMantleBlockGasLimit
        let gas_for_l1: u64 = op_req
            .as_ref()
            .gas
            .and_then(|g| g.try_into().ok())
            .map(|g: u64| g.min(GETH_MANTLE_RPC_GAS_CAP))
            .unwrap_or(GETH_MANTLE_RPC_GAS_CAP);
        let nonce_for_l1: u64 = op_req
            .as_ref()
            .nonce
            .and_then(|n| n.try_into().ok())
            .unwrap_or(0);

        // Align with geth: when chain has baseFee, CallDefaults sets MaxFeePerGas/MaxPriorityFeePerGas (0 if nil),
        // so ToTransaction builds EIP-1559. Use Legacy for L1 only when chain has no baseFee and request has no EIP-1559 fields.
        let has_base_fee = header.base_fee_per_gas().map_or(0, |g| g) > 0;
        let use_l1_legacy_path = !has_base_fee
            && op_req.as_ref().max_fee_per_gas.is_none()
            && op_req.as_ref().max_priority_fee_per_gas.is_none();

        // When block has no L1 info (e.g. earliest/genesis), use L1 fee 0 so we still return a total (align with geth)
        let (l1_data_fee, operator_fee) = match extract_l1_info(block.body()) {
            Ok(mut l1_block_info) => {
                // token_ratio is not in setL1BlockValues calldata; read from GasOracle contract state (align with receipt + geth)
                let state = self
                    .provider()
                    .state_by_block_hash(block.header().parent_hash())
                    .or_else(|_| self.provider().state_by_block_hash(block.header().hash_slow().into()));
                if let Ok(state) = state {
                    if let Ok(Some(ratio)) = state.storage(GAS_ORACLE_CONTRACT, TOKEN_RATIO_SLOT.into()) {
                        l1_block_info.token_ratio = ratio;
                    }
                }
                // Build tx for L1 cost with geth CallDefaults: gas=gas_cap (or request capped), nonce=0 when nil, fee fields 0 when nil.
                let encoded_for_l1 = {
                    let mut op_req_l1 = op_req.clone();
                    {
                        let r = op_req_l1.as_mut();
                        r.set_nonce(nonce_for_l1);
                        r.set_gas_limit(gas_for_l1);
                        if use_l1_legacy_path {
                            let gp = request.gas_price.unwrap_or(0);
                            r.set_gas_price(gp);
                            // Leave max_fee_per_gas and max_priority_fee_per_gas as None so build_typed_tx produces Legacy
                        } else {
                            // Match geth: when chain has baseFee, use EIP-1559 with 0/0 if request did not specify
                            let max_fee = request.max_fee_per_gas.unwrap_or(0);
                            let max_priority = request.max_priority_fee_per_gas.unwrap_or(0);
                            r.set_max_fee_per_gas(max_fee);
                            r.set_max_priority_fee_per_gas(max_priority);
                            // Do not set gas_price so encoder produces EIP-1559 (request with no gas_price keeps None)
                        }
                    }
                    let req = op_req_l1.as_ref();
                    let to_str = req
                        .to
                        .as_ref()
                        .map(|a| format!("{:?}", a))
                        .unwrap_or_else(|| "None".to_string());
                    let data_len = req.input.data.as_ref().map(|d| d.len()).unwrap_or(0);
                    let value_str = req.value.unwrap_or(U256::ZERO).to_string();
                    let chain_id = req.chain_id;
                    let gp = req.gas_price;
                    let mf = req.max_fee_per_gas;
                    let mp = req.max_priority_fee_per_gas;
                    let encoded = op_req_l1
                        .try_into_sim_tx()
                        .map_err(|_| {
                            ErrorObject::owned(
                                -32000,
                                "failed to build transaction for L1 fee",
                                None::<()>,
                            )
                        })?
                        .encoded_2718()
                        .to_vec();
                    // Debug: 与 geth 逐字段对比，字段名与 geth 一致便于 grep 对比。
                    info!(
                        tx_type = if use_l1_legacy_path { 0u8 } else { 2u8 },
                        nonce = nonce_for_l1,
                        gas = gas_for_l1,
                        to = %to_str,
                        value = %value_str,
                        data_len,
                        chain_id = ?chain_id,
                        gas_price = ?gp,
                        gas_fee_cap = ?mf,
                        gas_tip_cap = ?mp,
                        rlp_len = encoded.len(),
                        "eth_estimateTotalFee L1 tx [reth]"
                    );
                    encoded
                };

                let l1_data_fee = l1_block_info
                    .l1_tx_data_fee_for_estimate(
                        chain_spec,
                        header.timestamp(),
                        &encoded_for_l1,
                        false,
                        80, // geth adds 80 bytes for signature overhead
                    )
                    .map_err(|e| {
                        ErrorObject::owned(
                            -32000,
                            format!("failed to compute L1 data fee: {e}"),
                            None::<()>,
                        )
                    })?;
                let operator_fee = {
                    let scalar = l1_block_info
                        .operator_fee_scalar
                        .unwrap_or(U256::ZERO);
                    let constant = l1_block_info
                        .operator_fee_constant
                        .unwrap_or(U256::ZERO);
                    if scalar.is_zero() && constant.is_zero() {
                        U256::ZERO
                    } else {
                        gas_estimate
                            .saturating_mul(scalar)
                            .saturating_mul(U256::from(100))
                            .saturating_add(constant)
                    }
                };
                (l1_data_fee, operator_fee)
            }
            Err(_) => (U256::ZERO, U256::ZERO),
        };

        let total = l2_fee
            .saturating_add(l1_data_fee)
            .saturating_add(operator_fee);

        Ok(total)
    }
}
