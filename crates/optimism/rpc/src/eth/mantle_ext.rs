//! Mantle Eth API extension implementation.

use crate::{error::SequencerClientError, SequencerClient};
use alloy_consensus::BlockHeader;
use alloy_eips::{BlockId, BlockNumberOrTag, Encodable2718};
use alloy_network::TransactionBuilder;
use alloy_primitives::{Bytes, Sealable, TxKind, U256};
use alloy_rpc_types_eth::TransactionRequest;
use jsonrpsee::types::ErrorObject;
use jsonrpsee_core::RpcResult;
use op_revm::constants::{GAS_ORACLE_CONTRACT, TOKEN_RATIO_SLOT};
use reth_chainspec::{ChainSpecProvider, EthChainSpec};
use reth_mantle_forks::MantleHardforks;
use reth_optimism_evm::{extract_l1_info, RethL1BlockInfo};
use reth_primitives_traits::Block;
use reth_rpc_convert::TryIntoSimTx;
use reth_rpc_eth_api::{
    helpers::Call, EthApiServer, EthApiTypes, MantleEthApiExtServer, PreconfTxEvent, RpcBlock,
    RpcNodeCore,
};
use reth_rpc_server_types::result::invalid_params_rpc_err;
use reth_storage_api::{BlockNumReader, BlockReaderIdExt, StateProviderFactory};
use std::sync::Arc;

const GETH_MANTLE_RPC_GAS_CAP: u64 = 0x4000000000000;

fn estimate_total_fee_gas_price(
    request_gas_price: Option<u128>,
    request_max_fee_per_gas: Option<u128>,
    request_max_priority_fee_per_gas: Option<u128>,
    base_fee: U256,
    suggested_tip: U256,
) -> U256 {
    match (request_gas_price, request_max_fee_per_gas) {
        (Some(gas_price), _) if gas_price > 0 => U256::from(gas_price),
        (_, Some(max_fee)) if max_fee > 0 => {
            let tip = U256::from(request_max_priority_fee_per_gas.unwrap_or(0));
            base_fee.saturating_add(tip).min(U256::from(max_fee))
        }
        _ => base_fee.saturating_add(suggested_tip),
    }
}

fn capped_l1_gas_limit(request_gas: Option<u64>) -> u64 {
    request_gas.map(|gas| gas.min(GETH_MANTLE_RPC_GAS_CAP)).unwrap_or(GETH_MANTLE_RPC_GAS_CAP)
}

fn l1_nonce_or_default(request_nonce: Option<u64>) -> u64 {
    request_nonce.unwrap_or(0)
}

const fn should_use_l1_legacy_path(
    has_base_fee: bool,
    gas_price: Option<u128>,
    max_fee_per_gas: Option<u128>,
    max_priority_fee_per_gas: Option<u128>,
) -> bool {
    // Use Legacy tx encoding for L1 fee when no EIP-1559 fee fields are present AND either:
    // - the chain has no baseFee (pre-EIP-1559 chain), or
    // - the caller explicitly provided gasPrice only (matches geth behaviour: when CallDefaults
    //   sees gasPrice without maxFeePerGas/maxPriorityFeePerGas it builds a Legacy tx)
    max_fee_per_gas.is_none() &&
        max_priority_fee_per_gas.is_none() &&
        (!has_base_fee || gas_price.is_some())
}

fn ensure_create_kind_when_to_missing<T>(request: &mut T)
where
    T: AsRef<TransactionRequest> + AsMut<TransactionRequest>,
{
    if request.as_ref().to.is_none() {
        request.as_mut().set_kind(TxKind::Create);
    }
}

fn build_l1_fee_encoded_tx<T>(mut request: T) -> Result<Vec<u8>, ()>
where
    T: AsRef<TransactionRequest>
        + AsMut<TransactionRequest>
        + TryIntoSimTx<op_alloy_consensus::OpTxEnvelope>,
{
    ensure_create_kind_when_to_missing(&mut request);
    request.try_into_sim_tx().map(|tx| tx.encoded_2718()).map_err(|_| ())
}

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
    pub const fn new(
        provider: Provider,
        eth_api: Arc<EthApi>,
        sequencer_client: Option<SequencerClient>,
    ) -> Self {
        Self { provider, eth_api, sequencer_client }
    }

    #[inline]
    const fn provider(&self) -> &Provider {
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
    reth_rpc_eth_api::RpcTxReq<EthApi::NetworkTypes>:
        reth_rpc_convert::TryIntoSimTx<op_alloy_consensus::OpTxEnvelope> + Clone,
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
            .map_err(|e| {
                ErrorObject::owned(-32000, format!("failed to get block: {e}"), None::<()>)
            })?
            .ok_or_else(|| invalid_params_rpc_err("Block not found"))?;

        let header = block.header();
        let chain_spec = self.provider().chain_spec();

        if !chain_spec.is_arsia_active_at_timestamp(header.timestamp()) {
            return Err(ErrorObject::owned(
                -32000,
                "eth_estimateTotalFee is not supported for pre-Arsia blocks",
                None::<()>,
            ));
        }

        let mut req_value = serde_json::to_value(&request)
            .map_err(|e| ErrorObject::owned(-32000, format!("invalid request: {e}"), None::<()>))?;
        // Inject chainId when missing or null so build_typed_tx() can encode the tx for L1 fee
        if req_value.get("chainId").is_none_or(|v| v.is_null()) {
            req_value["chainId"] =
                serde_json::Value::String(format!("0x{:x}", chain_spec.chain().id()));
        }
        let mut op_req: reth_rpc_eth_api::RpcTxReq<EthApi::NetworkTypes> =
            serde_json::from_value(req_value).map_err(|e| {
                ErrorObject::owned(-32000, format!("invalid request: {e}"), None::<()>)
            })?;

        // Ensure chain_id is set so build_typed_tx() can encode the tx for L1 fee (JSON may omit or
        // use wrong key)
        if op_req.as_ref().chain_id.is_none() {
            op_req.as_mut().set_chain_id(chain_spec.chain().id());
        }

        let gas_estimate =
            self.eth_api().estimate_gas(op_req.clone(), Some(block_id), None).await.map_err(
                |e| {
                    ErrorObject::owned(
                        -32000,
                        format!("failed to estimate gas: {}", e.message()),
                        None::<()>,
                    )
                },
            )?;

        let base_fee = U256::from(header.base_fee_per_gas().unwrap_or(0));
        let suggested_tip = self.eth_api().max_priority_fee_per_gas().await.unwrap_or(U256::ZERO);
        let gas_price = estimate_total_fee_gas_price(
            request.gas_price,
            request.max_fee_per_gas,
            request.max_priority_fee_per_gas,
            base_fee,
            suggested_tip,
        );

        let l2_fee = gas_estimate.saturating_mul(gas_price);

        // Align with geth: tx for L1 cost is built with CallDefaults (gas cap when gas nil, nonce 0
        // when nil, fee fields 0 when nil).
        let gas_for_l1 = capped_l1_gas_limit(op_req.as_ref().gas);
        let nonce_for_l1 = l1_nonce_or_default(op_req.as_ref().nonce);

        // Align with geth: when chain has baseFee, CallDefaults sets
        // MaxFeePerGas/MaxPriorityFeePerGas (0 if nil), so ToTransaction builds EIP-1559.
        // Use Legacy for L1 only when chain has no baseFee and request has no EIP-1559 fields.
        let has_base_fee = header.base_fee_per_gas().map_or(0, |g| g) > 0;
        let use_l1_legacy_path = should_use_l1_legacy_path(
            has_base_fee,
            op_req.as_ref().gas_price,
            op_req.as_ref().max_fee_per_gas,
            op_req.as_ref().max_priority_fee_per_gas,
        );

        // When block has no L1 info (e.g. earliest/genesis), use L1 fee 0 so we still return a
        // total (align with geth)
        let (l1_data_fee, operator_fee) = match extract_l1_info(block.body()) {
            Ok(mut l1_block_info) => {
                // token_ratio is not in setL1BlockValues calldata; read from GasOracle contract
                // state (align with receipt + geth)
                let state = self
                    .provider()
                    .state_by_block_hash(block.header().parent_hash())
                    .or_else(|_| self.provider().state_by_block_hash(block.header().hash_slow()));
                if let Ok(state) = state &&
                    let Ok(Some(ratio)) =
                        state.storage(GAS_ORACLE_CONTRACT, TOKEN_RATIO_SLOT.into())
                {
                    l1_block_info.token_ratio = ratio;
                }
                // Build tx for L1 cost with geth CallDefaults: gas=gas_cap (or request capped),
                // nonce=0 when nil, fee fields 0 when nil.
                let encoded_for_l1 = {
                    let mut op_req_l1 = op_req.clone();
                    {
                        let r = op_req_l1.as_mut();
                        r.set_nonce(nonce_for_l1);
                        r.set_gas_limit(gas_for_l1);
                        if use_l1_legacy_path {
                            let gp = request.gas_price.unwrap_or(0);
                            r.set_gas_price(gp);
                            // Clear chain_id so the Legacy tx encodes with v=27
                            // (pre-EIP-155) instead of v=chain_id*2+35 (EIP-155).
                            // geth's ToTransaction() leaves V/R/S nil (RLP 0x80
                            // each = 3 bytes total). With chain_id set reth would
                            // encode v as a multi-byte integer (e.g. chain_id=5000
                            // → v=10035 → 3 RLP bytes), adding 2 extra bytes to
                            // the encoded tx, which changes FastLZ size and thus
                            // the L1 data fee.
                            r.chain_id = None;
                            // Leave max_fee_per_gas and max_priority_fee_per_gas as None so
                            // build_typed_tx produces Legacy
                        } else {
                            // Match geth: when chain has baseFee, use EIP-1559 with 0/0 if request
                            // did not specify
                            let max_fee = request.max_fee_per_gas.unwrap_or(0);
                            let max_priority = request.max_priority_fee_per_gas.unwrap_or(0);
                            r.set_max_fee_per_gas(max_fee);
                            r.set_max_priority_fee_per_gas(max_priority);
                            // Do not set gas_price so encoder produces EIP-1559 (request with no
                            // gas_price keeps None)
                        }
                    }
                    build_l1_fee_encoded_tx(op_req_l1).map_err(|_| {
                        ErrorObject::owned(
                            -32000,
                            "failed to build transaction for L1 fee",
                            None::<()>,
                        )
                    })?
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
                    let scalar = l1_block_info.operator_fee_scalar.unwrap_or(U256::ZERO);
                    let constant = l1_block_info.operator_fee_constant.unwrap_or(U256::ZERO);
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

        let total = l2_fee.saturating_add(l1_data_fee).saturating_add(operator_fee);

        Ok(total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::TxKind;
    use alloy_rpc_types_eth::TransactionInput;
    use op_alloy_rpc_types::OpTransactionRequest;

    #[test]
    fn estimate_total_fee_gas_price_prefers_explicit_gas_price() {
        let gas_price = estimate_total_fee_gas_price(
            Some(123),
            Some(999),
            Some(7),
            U256::from(10),
            U256::from(5),
        );
        assert_eq!(gas_price, U256::from(123));
    }

    #[test]
    fn estimate_total_fee_gas_price_uses_eip1559_cap() {
        let gas_price =
            estimate_total_fee_gas_price(None, Some(15), Some(10), U256::from(10), U256::from(5));
        assert_eq!(gas_price, U256::from(15));
    }

    #[test]
    fn estimate_total_fee_gas_price_uses_suggested_tip_fallback() {
        let gas_price =
            estimate_total_fee_gas_price(None, None, None, U256::from(10), U256::from(3));
        assert_eq!(gas_price, U256::from(13));
    }

    #[test]
    fn capped_l1_gas_limit_matches_geth_cap() {
        assert_eq!(capped_l1_gas_limit(None), GETH_MANTLE_RPC_GAS_CAP);
        assert_eq!(capped_l1_gas_limit(Some(21_000)), 21_000);
        assert_eq!(capped_l1_gas_limit(Some(GETH_MANTLE_RPC_GAS_CAP + 1)), GETH_MANTLE_RPC_GAS_CAP);
    }

    #[test]
    fn should_use_l1_legacy_path_only_without_base_fee_and_eip1559_fields() {
        // no base_fee, no gas_price, no EIP-1559 fields → Legacy
        assert!(should_use_l1_legacy_path(false, None, None, None));
        // has base_fee, no gas_price → NOT Legacy (EIP-1559 default)
        assert!(!should_use_l1_legacy_path(true, None, None, None));
        // max_fee_per_gas set → NOT Legacy
        assert!(!should_use_l1_legacy_path(false, None, Some(1), None));
        // max_priority_fee_per_gas set → NOT Legacy
        assert!(!should_use_l1_legacy_path(false, None, None, Some(1)));
    }

    #[test]
    fn build_l1_fee_encoded_tx_supports_contract_creation_requests() {
        let request: OpTransactionRequest = TransactionRequest {
            chain_id: Some(5000),
            from: Some(alloy_primitives::address!("f39fd6e51aad88f6f4ce6ab8827279cfffb92266")),
            gas: Some(100_000),
            nonce: Some(0),
            max_fee_per_gas: Some(10),
            max_priority_fee_per_gas: Some(1),
            input: TransactionInput::from(alloy_primitives::bytes!(
                "600a600c600039600a6000f3602a60005260206000f3"
            )),
            ..Default::default()
        }
        .into();

        let encoded = build_l1_fee_encoded_tx(request).expect("contract creation should encode");
        assert!(!encoded.is_empty(), "encoded tx should not be empty");
    }

    /// Regression test: Legacy tx for L1 fee must NOT include `chain_id` in the
    /// signature v value.  When `chain_id` is present, alloy encodes
    /// v = `chain_id`*2 + 35 (EIP-155) which adds extra bytes compared to geth
    /// (geth leaves V/R/S nil → 0x80 each).  Clearing `chain_id` produces
    /// v = 27 (pre-EIP-155, 1 byte in RLP), matching geth's byte length.
    #[test]
    fn legacy_l1_fee_encoded_tx_has_no_chain_id_in_signature() {
        // Simulate what estimate_total_fee does in the Legacy path:
        // 1. Start with a request that has chain_id (injected earlier in the flow)
        // 2. Set gas_price (Legacy) and clear chain_id
        let mut request: OpTransactionRequest = TransactionRequest {
            chain_id: Some(5000), // injected by estimate_total_fee
            from: Some(alloy_primitives::address!("f39fd6e51aad88f6f4ce6ab8827279cfffb92266")),
            gas: Some(GETH_MANTLE_RPC_GAS_CAP),
            nonce: Some(0),
            gas_price: Some(1_000_000_000), // Legacy: gasPrice only
            to: Some(TxKind::Call(alloy_primitives::Address::ZERO)),
            input: TransactionInput::from(alloy_primitives::bytes!(
                "000102030405060708090a0b0c0d0e0f"
            )),
            ..Default::default()
        }
        .into();

        // Clear chain_id as the fix does
        request.as_mut().chain_id = None;

        let encoded_no_chain =
            build_l1_fee_encoded_tx(request).expect("legacy tx should encode without chain_id");

        // Now build the same request WITH chain_id to show the difference
        let request_with_chain: OpTransactionRequest = TransactionRequest {
            chain_id: Some(5000),
            from: Some(alloy_primitives::address!("f39fd6e51aad88f6f4ce6ab8827279cfffb92266")),
            gas: Some(GETH_MANTLE_RPC_GAS_CAP),
            nonce: Some(0),
            gas_price: Some(1_000_000_000),
            to: Some(TxKind::Call(alloy_primitives::Address::ZERO)),
            input: TransactionInput::from(alloy_primitives::bytes!(
                "000102030405060708090a0b0c0d0e0f"
            )),
            ..Default::default()
        }
        .into();

        let encoded_with_chain = build_l1_fee_encoded_tx(request_with_chain)
            .expect("legacy tx should encode with chain_id");

        // The encoding WITHOUT chain_id must be shorter (v=27 is 1 byte vs
        // v=chain_id*2+35=10035 which is 3 bytes in RLP, plus the RLP list
        // header may also grow).
        assert!(
            encoded_no_chain.len() < encoded_with_chain.len(),
            "Legacy tx without chain_id ({} bytes) should be shorter than with chain_id ({} bytes) \
             due to EIP-155 v encoding overhead",
            encoded_no_chain.len(),
            encoded_with_chain.len(),
        );

        // Verify v byte: without chain_id, the v value should be 27 (0x1b),
        // which is the pre-EIP-155 recovery id.  The v byte sits just after
        // the last data field in the RLP list.
        // For Legacy RLP: [nonce, gasPrice, gasLimit, to, value, data, v, r, s]
        // v=27 encodes as single byte 0x1b.
        assert!(
            encoded_no_chain.windows(1).any(|w| w == [0x1b]),
            "encoded legacy tx without chain_id should contain v=27 (0x1b)"
        );
        // With chain_id=5000, v=10035=0x2733.
        assert!(
            encoded_with_chain.windows(2).any(|w| w == [0x27, 0x33]),
            "encoded legacy tx with chain_id should contain v=10035 (0x2733)"
        );
    }

    /// Verify the Legacy path properly matches geth's `should_use_l1_legacy_path`
    /// when gasPrice is explicitly set alongside a chain that has baseFee.
    #[test]
    fn should_use_l1_legacy_path_with_explicit_gas_price_and_base_fee() {
        // gasPrice set, no EIP-1559 fields, chain has baseFee → Legacy
        assert!(should_use_l1_legacy_path(true, Some(1_000_000_000), None, None));
        // gasPrice set, maxFeePerGas also set → NOT Legacy (EIP-1559 takes precedence)
        assert!(!should_use_l1_legacy_path(true, Some(1_000_000_000), Some(2_000_000_000), None));
    }

    #[test]
    fn ensure_create_kind_only_when_to_is_missing() {
        let mut create_request: OpTransactionRequest = TransactionRequest {
            chain_id: Some(5000),
            gas: Some(100_000),
            nonce: Some(0),
            max_fee_per_gas: Some(10),
            max_priority_fee_per_gas: Some(1),
            ..Default::default()
        }
        .into();
        ensure_create_kind_when_to_missing(&mut create_request);
        assert_eq!(create_request.as_ref().to, Some(TxKind::Create));

        let to = alloy_primitives::address!("70997970c51812dc3a010c7d01b50e0d17dc79c8");
        let mut call_request: OpTransactionRequest = TransactionRequest {
            chain_id: Some(5000),
            gas: Some(100_000),
            nonce: Some(0),
            max_fee_per_gas: Some(10),
            max_priority_fee_per_gas: Some(1),
            to: Some(TxKind::Call(to)),
            ..Default::default()
        }
        .into();
        ensure_create_kind_when_to_missing(&mut call_request);
        assert_eq!(call_request.as_ref().to, Some(TxKind::Call(to)));
    }
}
