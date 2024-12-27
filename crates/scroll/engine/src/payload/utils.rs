use alloc::{sync::Arc, vec::Vec};
use alloy_consensus::EMPTY_OMMER_ROOT_HASH;
use alloy_eips::eip2718::Decodable2718;
use alloy_rlp::BufMut;
use alloy_rpc_types_engine::{
    ExecutionPayload, ExecutionPayloadSidecar, ExecutionPayloadV1, ExecutionPayloadV2,
    ExecutionPayloadV3, PayloadError,
};
use reth_primitives::{proofs, Block, BlockBody, Header};
use reth_scroll_chainspec::ScrollChainSpec;
use reth_scroll_forks::ScrollHardfork;

/// Tries to create a new unsealed block from the given payload, sidecar and chain specification.
pub fn try_into_block<T: Decodable2718>(
    value: ExecutionPayload,
    sidecar: &ExecutionPayloadSidecar,
    chainspec: Arc<ScrollChainSpec>,
) -> Result<Block<T>, PayloadError> {
    let mut base_payload = match value {
        ExecutionPayload::V1(payload) => try_payload_v1_to_block(payload, chainspec)?,
        ExecutionPayload::V2(payload) => try_payload_v2_to_block(payload, chainspec)?,
        ExecutionPayload::V3(payload) => try_payload_v3_to_block(payload, chainspec)?,
    };

    base_payload.header.parent_beacon_block_root = sidecar.parent_beacon_block_root();
    base_payload.header.requests_hash = sidecar.requests_hash();

    Ok(base_payload)
}

/// Tries to convert an [`ExecutionPayloadV1`] to [`Block`].
fn try_payload_v1_to_block<T: Decodable2718>(
    payload: ExecutionPayloadV1,
    chainspec: Arc<ScrollChainSpec>,
) -> Result<Block<T>, PayloadError> {
    // WARNING: It’s allowed for a base fee in EIP1559 to increase unbounded. We assume that
    // it will fit in an u64. This is not always necessarily true, although it is extremely
    // unlikely not to be the case, a u64 maximum would have 2^64 which equates to 18 ETH per
    // gas.
    let basefee = chainspec
        .is_fork_active_at_block(ScrollHardfork::Curie, payload.block_number)
        .then_some(payload.base_fee_per_gas)
        .map(|b| b.try_into())
        .transpose()
        .map_err(|_| PayloadError::BaseFee(payload.base_fee_per_gas))?;

    let transactions = payload
        .transactions
        .iter()
        .map(|tx| {
            let mut buf = tx.as_ref();

            let tx = T::decode_2718(&mut buf).map_err(alloy_rlp::Error::from)?;

            if !buf.is_empty() {
                return Err(alloy_rlp::Error::UnexpectedLength);
            }

            Ok(tx)
        })
        .collect::<Result<Vec<_>, _>>()?;

    // Reuse the encoded bytes for root calculation
    let transactions_root =
        proofs::ordered_trie_root_with_encoder(&payload.transactions, |item, buf| {
            buf.put_slice(item)
        });

    let header = Header {
        parent_hash: payload.parent_hash,
        beneficiary: payload.fee_recipient,
        state_root: payload.state_root,
        transactions_root,
        receipts_root: payload.receipts_root,
        withdrawals_root: None,
        logs_bloom: payload.logs_bloom,
        number: payload.block_number,
        gas_limit: payload.gas_limit,
        gas_used: payload.gas_used,
        timestamp: payload.timestamp,
        mix_hash: payload.prev_randao,
        base_fee_per_gas: basefee,
        blob_gas_used: None,
        excess_blob_gas: None,
        parent_beacon_block_root: None,
        requests_hash: None,
        extra_data: payload.extra_data,
        // Defaults
        ommers_hash: EMPTY_OMMER_ROOT_HASH,
        difficulty: Default::default(),
        nonce: Default::default(),
        target_blobs_per_block: None,
    };

    Ok(Block { header, body: BlockBody { transactions, ..Default::default() } })
}

/// Tries to convert an [`ExecutionPayloadV2`] to [`Block`].
fn try_payload_v2_to_block<T: Decodable2718>(
    payload: ExecutionPayloadV2,
    chainspec: Arc<ScrollChainSpec>,
) -> Result<Block<T>, PayloadError> {
    // this performs the same conversion as the underlying V1 payload, but calculates the
    // withdrawals root and adds withdrawals
    let mut base_sealed_block = try_payload_v1_to_block(payload.payload_inner, chainspec)?;
    let withdrawals_root = proofs::calculate_withdrawals_root(&payload.withdrawals);
    base_sealed_block.body.withdrawals = Some(payload.withdrawals.into());
    base_sealed_block.header.withdrawals_root = Some(withdrawals_root);
    Ok(base_sealed_block)
}

/// Tries to convert an [`ExecutionPayloadV3`] to [`Block`].
fn try_payload_v3_to_block<T: Decodable2718>(
    payload: ExecutionPayloadV3,
    chainspec: Arc<ScrollChainSpec>,
) -> Result<Block<T>, PayloadError> {
    // this performs the same conversion as the underlying V2 payload, but inserts the blob gas
    // used and excess blob gas
    let mut base_block = try_payload_v2_to_block(payload.payload_inner, chainspec)?;

    base_block.header.blob_gas_used = Some(payload.blob_gas_used);
    base_block.header.excess_blob_gas = Some(payload.excess_blob_gas);

    Ok(base_block)
}
