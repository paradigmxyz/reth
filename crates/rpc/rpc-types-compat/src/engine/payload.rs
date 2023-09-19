//! Standalone Conversion Functions for Handling Different Versions of Execution Payloads in
//! Ethereum's Engine
use reth_primitives::{
    constants::{MAXIMUM_EXTRA_DATA_SIZE, MIN_PROTOCOL_BASE_FEE_U256},
    proofs::{self, EMPTY_LIST_HASH},
    Block, Header, SealedBlock, TransactionSigned, UintTryTo, Withdrawal, H256, U256,
};
use reth_rlp::Decodable;
use reth_rpc_types::engine::{
    payload::{ExecutionPayloadBodyV1, ExecutionPayloadFieldV2, ExecutionPayloadInputV2},
    ExecutionPayload, ExecutionPayloadV1, ExecutionPayloadV2, ExecutionPayloadV3, PayloadError,
};

/// Converts [ExecutionPayloadV1] to [Block]
pub fn try_payload_v1_to_block(payload: ExecutionPayloadV1) -> Result<Block, PayloadError> {
    if payload.extra_data.len() > MAXIMUM_EXTRA_DATA_SIZE {
        return Err(PayloadError::ExtraData(payload.extra_data))
    }

    if payload.base_fee_per_gas < MIN_PROTOCOL_BASE_FEE_U256 {
        return Err(PayloadError::BaseFee(payload.base_fee_per_gas))
    }

    let transactions = payload
        .transactions
        .iter()
        .map(|tx| TransactionSigned::decode(&mut tx.as_ref()))
        .collect::<Result<Vec<_>, _>>()?;
    let transactions_root = proofs::calculate_transaction_root(&transactions);

    let header = Header {
        parent_hash: payload.parent_hash,
        beneficiary: payload.fee_recipient,
        state_root: payload.state_root,
        transactions_root,
        receipts_root: payload.receipts_root,
        withdrawals_root: None,
        logs_bloom: payload.logs_bloom,
        number: payload.block_number.as_u64(),
        gas_limit: payload.gas_limit.as_u64(),
        gas_used: payload.gas_used.as_u64(),
        timestamp: payload.timestamp.as_u64(),
        mix_hash: payload.prev_randao,
        base_fee_per_gas: Some(
            payload
                .base_fee_per_gas
                .uint_try_to()
                .map_err(|_| PayloadError::BaseFee(payload.base_fee_per_gas))?,
        ),
        blob_gas_used: None,
        excess_blob_gas: None,
        parent_beacon_block_root: None,
        extra_data: payload.extra_data,
        // Defaults
        ommers_hash: EMPTY_LIST_HASH,
        difficulty: Default::default(),
        nonce: Default::default(),
    };

    Ok(Block { header, body: transactions, withdrawals: None, ommers: Default::default() })
}

/// Converts [ExecutionPayloadV2] to [Block]
pub fn try_payload_v2_to_block(payload: ExecutionPayloadV2) -> Result<Block, PayloadError> {
    // this performs the same conversion as the underlying V1 payload, but calculates the
    // withdrawals root and adds withdrawals
    let mut base_sealed_block = try_payload_v1_to_block(payload.payload_inner)?;
    let withdrawals: Vec<_> = payload
        .withdrawals
        .iter()
        .map(|w| convert_standalonewithdraw_to_withdrawal(w.clone()))
        .collect();
    let withdrawals_root = proofs::calculate_withdrawals_root(&withdrawals);
    base_sealed_block.withdrawals = Some(withdrawals);
    base_sealed_block.header.withdrawals_root = Some(withdrawals_root);
    Ok(base_sealed_block)
}

/// Converts [ExecutionPayloadV3] to [Block]
pub fn try_payload_v3_to_block(payload: ExecutionPayloadV3) -> Result<Block, PayloadError> {
    // this performs the same conversion as the underlying V2 payload, but inserts the blob gas
    // used and excess blob gas
    let mut base_block = try_payload_v2_to_block(payload.payload_inner)?;

    base_block.header.blob_gas_used = Some(payload.blob_gas_used.as_u64());
    base_block.header.excess_blob_gas = Some(payload.excess_blob_gas.as_u64());

    Ok(base_block)
}

/// Converts [SealedBlock] to [ExecutionPayload]
pub fn try_block_to_payload(value: SealedBlock) -> ExecutionPayload {
    if value.header.parent_beacon_block_root.is_some() {
        // block with parent beacon block root: V3
        ExecutionPayload::V3(try_block_to_payload_v3(value))
    } else if value.withdrawals.is_some() {
        // block with withdrawals: V2
        ExecutionPayload::V2(try_block_to_payload_v2(value))
    } else {
        // otherwise V1
        ExecutionPayload::V1(try_block_to_payload_v1(value))
    }
}

/// Converts [SealedBlock] to [ExecutionPayloadV1]
pub fn try_block_to_payload_v1(value: SealedBlock) -> ExecutionPayloadV1 {
    let transactions = value
        .body
        .iter()
        .map(|tx| {
            let mut encoded = Vec::new();
            tx.encode_enveloped(&mut encoded);
            encoded.into()
        })
        .collect();
    ExecutionPayloadV1 {
        parent_hash: value.parent_hash,
        fee_recipient: value.beneficiary,
        state_root: value.state_root,
        receipts_root: value.receipts_root,
        logs_bloom: value.logs_bloom,
        prev_randao: value.mix_hash,
        block_number: value.number.into(),
        gas_limit: value.gas_limit.into(),
        gas_used: value.gas_used.into(),
        timestamp: value.timestamp.into(),
        extra_data: value.extra_data.clone(),
        base_fee_per_gas: U256::from(value.base_fee_per_gas.unwrap_or_default()),
        block_hash: value.hash(),
        transactions,
    }
}

/// Converts [SealedBlock] to [ExecutionPayloadV2]
pub fn try_block_to_payload_v2(value: SealedBlock) -> ExecutionPayloadV2 {
    let transactions = value
        .body
        .iter()
        .map(|tx| {
            let mut encoded = Vec::new();
            tx.encode_enveloped(&mut encoded);
            encoded.into()
        })
        .collect();
    let standalone_withdrawals: Vec<reth_rpc_types::engine::payload::Withdrawal> = value
        .withdrawals
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(convert_withdrawal_to_standalonewithdraw)
        .collect();

    ExecutionPayloadV2 {
        payload_inner: ExecutionPayloadV1 {
            parent_hash: value.parent_hash,
            fee_recipient: value.beneficiary,
            state_root: value.state_root,
            receipts_root: value.receipts_root,
            logs_bloom: value.logs_bloom,
            prev_randao: value.mix_hash,
            block_number: value.number.into(),
            gas_limit: value.gas_limit.into(),
            gas_used: value.gas_used.into(),
            timestamp: value.timestamp.into(),
            extra_data: value.extra_data.clone(),
            base_fee_per_gas: U256::from(value.base_fee_per_gas.unwrap_or_default()),
            block_hash: value.hash(),
            transactions,
        },
        withdrawals: standalone_withdrawals,
    }
}

/// Converts [SealedBlock] to [ExecutionPayloadV3]
pub fn try_block_to_payload_v3(value: SealedBlock) -> ExecutionPayloadV3 {
    let transactions = value
        .body
        .iter()
        .map(|tx| {
            let mut encoded = Vec::new();
            tx.encode_enveloped(&mut encoded);
            encoded.into()
        })
        .collect();

    let withdrawals: Vec<reth_rpc_types::engine::payload::Withdrawal> = value
        .withdrawals
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(convert_withdrawal_to_standalonewithdraw)
        .collect();

    ExecutionPayloadV3 {
        payload_inner: ExecutionPayloadV2 {
            payload_inner: ExecutionPayloadV1 {
                parent_hash: value.parent_hash,
                fee_recipient: value.beneficiary,
                state_root: value.state_root,
                receipts_root: value.receipts_root,
                logs_bloom: value.logs_bloom,
                prev_randao: value.mix_hash,
                block_number: value.number.into(),
                gas_limit: value.gas_limit.into(),
                gas_used: value.gas_used.into(),
                timestamp: value.timestamp.into(),
                extra_data: value.extra_data.clone(),
                base_fee_per_gas: U256::from(value.base_fee_per_gas.unwrap_or_default()),
                block_hash: value.hash(),
                transactions,
            },
            withdrawals,
        },

        blob_gas_used: value.blob_gas_used.unwrap_or_default().into(),
        excess_blob_gas: value.excess_blob_gas.unwrap_or_default().into(),
    }
}

/// Converts [SealedBlock] to [ExecutionPayloadFieldV2]
pub fn convert_block_to_payload_field_v2(value: SealedBlock) -> ExecutionPayloadFieldV2 {
    // if there are withdrawals, return V2
    if value.withdrawals.is_some() {
        ExecutionPayloadFieldV2::V2(try_block_to_payload_v2(value))
    } else {
        ExecutionPayloadFieldV2::V1(try_block_to_payload_v1(value))
    }
}

/// Converts [ExecutionPayloadFieldV2] to [ExecutionPayload]
pub fn convert_payload_field_v2_to_payload(value: ExecutionPayloadFieldV2) -> ExecutionPayload {
    match value {
        ExecutionPayloadFieldV2::V1(payload) => ExecutionPayload::V1(payload),
        ExecutionPayloadFieldV2::V2(payload) => ExecutionPayload::V2(payload),
    }
}

/// Converts [ExecutionPayloadInputV2] to [ExecutionPayload]
pub fn convert_payload_input_v2_to_payload(value: ExecutionPayloadInputV2) -> ExecutionPayload {
    match value.withdrawals {
        Some(withdrawals) => ExecutionPayload::V2(ExecutionPayloadV2 {
            payload_inner: value.execution_payload,
            withdrawals,
        }),
        None => ExecutionPayload::V1(value.execution_payload),
    }
}

/// Converts [SealedBlock] to [ExecutionPayloadInputV2]
pub fn convert_block_to_payload_input_v2(value: SealedBlock) -> ExecutionPayloadInputV2 {
    let withdraw = value.withdrawals.clone().map(|withdrawals| {
        withdrawals.into_iter().map(convert_withdrawal_to_standalonewithdraw).collect::<Vec<_>>()
    });
    ExecutionPayloadInputV2 {
        withdrawals: withdraw,
        execution_payload: try_block_to_payload_v1(value),
    }
}

/// Tries to create a new block from the given payload and optional parent beacon block root.
/// Perform additional validation of `extra_data` and `base_fee_per_gas` fields.
///
/// NOTE: The log bloom is assumed to be validated during serialization.
/// NOTE: Empty ommers, nonce and difficulty values are validated upon computing block hash and
/// comparing the value with `payload.block_hash`.
///
/// See <https://github.com/ethereum/go-ethereum/blob/79a478bb6176425c2400e949890e668a3d9a3d05/core/beacon/types.go#L145>
pub fn try_into_sealed_block(
    value: ExecutionPayload,
    parent_beacon_block_root: Option<H256>,
) -> Result<SealedBlock, PayloadError> {
    let block_hash = value.block_hash();
    let mut base_payload = match value {
        ExecutionPayload::V1(payload) => try_payload_v1_to_block(payload)?,
        ExecutionPayload::V2(payload) => try_payload_v2_to_block(payload)?,
        ExecutionPayload::V3(payload) => try_payload_v3_to_block(payload)?,
    };

    base_payload.header.parent_beacon_block_root = parent_beacon_block_root;

    let payload = base_payload.seal_slow();

    if block_hash != payload.hash() {
        return Err(PayloadError::BlockHash { execution: payload.hash(), consensus: block_hash })
    }
    Ok(payload)
}

/// Converts [Withdrawal] to [reth_rpc_types::engine::payload::Withdrawal]
pub fn convert_withdrawal_to_standalonewithdraw(
    withdrawal: Withdrawal,
) -> reth_rpc_types::engine::payload::Withdrawal {
    reth_rpc_types::engine::payload::Withdrawal {
        index: withdrawal.index,
        validator_index: withdrawal.validator_index,
        address: withdrawal.address,
        amount: withdrawal.amount,
    }
}

/// Converts [reth_rpc_types::engine::payload::Withdrawal] to [Withdrawal]
pub fn convert_standalonewithdraw_to_withdrawal(
    standalone: reth_rpc_types::engine::payload::Withdrawal,
) -> Withdrawal {
    Withdrawal {
        index: standalone.index,
        validator_index: standalone.validator_index,
        address: standalone.address,
        amount: standalone.amount,
    }
}

/// Converts [Block] to [ExecutionPayloadBodyV1]
pub fn convert_to_payload_body_v1(value: Block) -> ExecutionPayloadBodyV1 {
    let transactions = value.body.into_iter().map(|tx| {
        let mut out = Vec::new();
        tx.encode_enveloped(&mut out);
        out.into()
    });
    let withdraw: Option<Vec<reth_rpc_types::engine::payload::Withdrawal>> =
        value.withdrawals.map(|withdrawals| {
            withdrawals
                .into_iter()
                .map(convert_withdrawal_to_standalonewithdraw)
                .collect::<Vec<_>>()
        });
    ExecutionPayloadBodyV1 { transactions: transactions.collect(), withdrawals: withdraw }
}
