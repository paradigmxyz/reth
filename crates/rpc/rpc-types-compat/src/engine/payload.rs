//! Standalone Conversion Functions for Handling Different Versions of Execution Payloads in
//! Ethereum's Engine

use reth_primitives::{
    constants::{EMPTY_OMMER_ROOT_HASH, MAXIMUM_EXTRA_DATA_SIZE, MIN_PROTOCOL_BASE_FEE_U256},
    proofs::{self},
    Block, Header, SealedBlock, TransactionSigned, UintTryTo, Withdrawal, Withdrawals, B256, U256,
};
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
        .into_iter()
        .map(|tx| TransactionSigned::decode_enveloped(&mut tx.as_ref()))
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
        number: payload.block_number,
        gas_limit: payload.gas_limit,
        gas_used: payload.gas_used,
        timestamp: payload.timestamp,
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
        ommers_hash: EMPTY_OMMER_ROOT_HASH,
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
    let withdrawals = Withdrawals::new(
        payload.withdrawals.iter().map(|w| convert_standalone_withdraw_to_withdrawal(*w)).collect(),
    );
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

    base_block.header.blob_gas_used = Some(payload.blob_gas_used);
    base_block.header.excess_blob_gas = Some(payload.excess_blob_gas);

    Ok(base_block)
}

/// Converts [SealedBlock] to [ExecutionPayload]
pub fn try_block_to_payload(value: SealedBlock) -> ExecutionPayload {
    if value.header.parent_beacon_block_root.is_some() {
        // block with parent beacon block root: V3
        ExecutionPayload::V3(block_to_payload_v3(value))
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
    let transactions = value.raw_transactions();
    ExecutionPayloadV1 {
        parent_hash: value.parent_hash,
        fee_recipient: value.beneficiary,
        state_root: value.state_root,
        receipts_root: value.receipts_root,
        logs_bloom: value.logs_bloom,
        prev_randao: value.mix_hash,
        block_number: value.number,
        gas_limit: value.gas_limit,
        gas_used: value.gas_used,
        timestamp: value.timestamp,
        extra_data: value.extra_data.clone(),
        base_fee_per_gas: U256::from(value.base_fee_per_gas.unwrap_or_default()),
        block_hash: value.hash(),
        transactions,
    }
}

/// Converts [SealedBlock] to [ExecutionPayloadV2]
pub fn try_block_to_payload_v2(value: SealedBlock) -> ExecutionPayloadV2 {
    let transactions = value.raw_transactions();
    let standalone_withdrawals: Vec<reth_rpc_types::withdrawal::Withdrawal> = value
        .withdrawals
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(convert_withdrawal_to_standalone_withdraw)
        .collect();

    ExecutionPayloadV2 {
        payload_inner: ExecutionPayloadV1 {
            parent_hash: value.parent_hash,
            fee_recipient: value.beneficiary,
            state_root: value.state_root,
            receipts_root: value.receipts_root,
            logs_bloom: value.logs_bloom,
            prev_randao: value.mix_hash,
            block_number: value.number,
            gas_limit: value.gas_limit,
            gas_used: value.gas_used,
            timestamp: value.timestamp,
            extra_data: value.extra_data.clone(),
            base_fee_per_gas: U256::from(value.base_fee_per_gas.unwrap_or_default()),
            block_hash: value.hash(),
            transactions,
        },
        withdrawals: standalone_withdrawals,
    }
}

/// Converts [SealedBlock] to [ExecutionPayloadV3]
pub fn block_to_payload_v3(value: SealedBlock) -> ExecutionPayloadV3 {
    let transactions = value.raw_transactions();

    let withdrawals: Vec<reth_rpc_types::withdrawal::Withdrawal> = value
        .withdrawals
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(convert_withdrawal_to_standalone_withdraw)
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
                block_number: value.number,
                gas_limit: value.gas_limit,
                gas_used: value.gas_used,
                timestamp: value.timestamp,
                extra_data: value.extra_data.clone(),
                base_fee_per_gas: U256::from(value.base_fee_per_gas.unwrap_or_default()),
                block_hash: value.hash(),
                transactions,
            },
            withdrawals,
        },

        blob_gas_used: value.blob_gas_used.unwrap_or_default(),
        excess_blob_gas: value.excess_blob_gas.unwrap_or_default(),
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
        withdrawals.into_iter().map(convert_withdrawal_to_standalone_withdraw).collect::<Vec<_>>()
    });
    ExecutionPayloadInputV2 {
        withdrawals: withdraw,
        execution_payload: try_block_to_payload_v1(value),
    }
}

/// Tries to create a new block (without a block hash) from the given payload and optional parent
/// beacon block root.
/// Performs additional validation of `extra_data` and `base_fee_per_gas` fields.
///
/// NOTE: The log bloom is assumed to be validated during serialization.
///
/// See <https://github.com/ethereum/go-ethereum/blob/79a478bb6176425c2400e949890e668a3d9a3d05/core/beacon/types.go#L145>
pub fn try_into_block(
    value: ExecutionPayload,
    parent_beacon_block_root: Option<B256>,
) -> Result<Block, PayloadError> {
    let mut base_payload = match value {
        ExecutionPayload::V1(payload) => try_payload_v1_to_block(payload)?,
        ExecutionPayload::V2(payload) => try_payload_v2_to_block(payload)?,
        ExecutionPayload::V3(payload) => try_payload_v3_to_block(payload)?,
    };

    base_payload.header.parent_beacon_block_root = parent_beacon_block_root;

    Ok(base_payload)
}

/// Tries to create a new block from the given payload and optional parent beacon block root.
///
/// NOTE: Empty ommers, nonce and difficulty values are validated upon computing block hash and
/// comparing the value with `payload.block_hash`.
///
/// Uses [try_into_block] to convert from the [ExecutionPayload] to [Block] and seals the block
/// with its hash.
///
/// Uses [validate_block_hash] to validate the payload block hash and ultimately return the
/// [SealedBlock].
pub fn try_into_sealed_block(
    payload: ExecutionPayload,
    parent_beacon_block_root: Option<B256>,
) -> Result<SealedBlock, PayloadError> {
    let block_hash = payload.block_hash();
    let base_payload = try_into_block(payload, parent_beacon_block_root)?;

    // validate block hash and return
    validate_block_hash(block_hash, base_payload)
}

/// Takes the expected block hash and [Block], validating the block and converting it into a
/// [SealedBlock].
///
/// If the provided block hash does not match the block hash computed from the provided block, this
/// returns [PayloadError::BlockHash].
#[inline]
pub fn validate_block_hash(
    expected_block_hash: B256,
    block: Block,
) -> Result<SealedBlock, PayloadError> {
    let sealed_block = block.seal_slow();
    if expected_block_hash != sealed_block.hash() {
        return Err(PayloadError::BlockHash {
            execution: sealed_block.hash(),
            consensus: expected_block_hash,
        })
    }

    Ok(sealed_block)
}

/// Converts [Withdrawal] to [reth_rpc_types::withdrawal::Withdrawal]
pub fn convert_withdrawal_to_standalone_withdraw(
    withdrawal: Withdrawal,
) -> reth_rpc_types::withdrawal::Withdrawal {
    reth_rpc_types::withdrawal::Withdrawal {
        index: withdrawal.index,
        validator_index: withdrawal.validator_index,
        address: withdrawal.address,
        amount: withdrawal.amount,
    }
}

/// Converts [reth_rpc_types::withdrawal::Withdrawal] to [Withdrawal]
pub fn convert_standalone_withdraw_to_withdrawal(
    standalone: reth_rpc_types::withdrawal::Withdrawal,
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
    let withdraw: Option<Vec<reth_rpc_types::withdrawal::Withdrawal>> =
        value.withdrawals.map(|withdrawals| {
            withdrawals
                .into_iter()
                .map(convert_withdrawal_to_standalone_withdraw)
                .collect::<Vec<_>>()
        });
    ExecutionPayloadBodyV1 { transactions: transactions.collect(), withdrawals: withdraw }
}

/// Transforms a [SealedBlock] into a [ExecutionPayloadV1]
pub fn execution_payload_from_sealed_block(value: SealedBlock) -> ExecutionPayloadV1 {
    let transactions = value.raw_transactions();
    ExecutionPayloadV1 {
        parent_hash: value.parent_hash,
        fee_recipient: value.beneficiary,
        state_root: value.state_root,
        receipts_root: value.receipts_root,
        logs_bloom: value.logs_bloom,
        prev_randao: value.mix_hash,
        block_number: value.number,
        gas_limit: value.gas_limit,
        gas_used: value.gas_used,
        timestamp: value.timestamp,
        extra_data: value.extra_data.clone(),
        base_fee_per_gas: U256::from(value.base_fee_per_gas.unwrap_or_default()),
        block_hash: value.hash(),
        transactions,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        block_to_payload_v3, try_into_block, try_payload_v3_to_block, validate_block_hash,
    };
    use reth_primitives::{b256, hex, Bytes, U256};
    use reth_rpc_types::{
        engine::{CancunPayloadFields, ExecutionPayloadV3},
        ExecutionPayload, ExecutionPayloadV1, ExecutionPayloadV2,
    };

    #[test]
    fn roundtrip_payload_to_block() {
        let first_transaction_raw = Bytes::from_static(&hex!("02f9017a8501a1f0ff438211cc85012a05f2008512a05f2000830249f094d5409474fd5a725eab2ac9a8b26ca6fb51af37ef80b901040cc7326300000000000000000000000000000000000000000000000000000000000000a000000000000000000000000000000000000000000000001bdd2ed4b616c800000000000000000000000000001e9ee781dd4b97bdef92e5d1785f73a1f931daa20000000000000000000000007a40026a3b9a41754a95eec8c92c6b99886f440c000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000020000000000000000000000009ae80eb647dd09968488fa1d7e412bf8558a0b7a0000000000000000000000000f9815537d361cb02befd9918c95c97d4d8a4a2bc001a0ba8f1928bb0efc3fcd01524a2039a9a2588fa567cd9a7cc18217e05c615e9d69a0544bfd11425ac7748e76b3795b57a5563e2b0eff47b5428744c62ff19ccfc305")[..]);
        let second_transaction_raw = Bytes::from_static(&hex!("03f901388501a1f0ff430c843b9aca00843b9aca0082520894e7249813d8ccf6fa95a2203f46a64166073d58878080c005f8c6a00195f6dff17753fc89b60eac6477026a805116962c9e412de8015c0484e661c1a001aae314061d4f5bbf158f15d9417a238f9589783f58762cd39d05966b3ba2fba0013f5be9b12e7da06f0dd11a7bdc4e0db8ef33832acc23b183bd0a2c1408a757a0019d9ac55ea1a615d92965e04d960cb3be7bff121a381424f1f22865bd582e09a001def04412e76df26fefe7b0ed5e10580918ae4f355b074c0cfe5d0259157869a0011c11a415db57e43db07aef0de9280b591d65ca0cce36c7002507f8191e5d4a80a0c89b59970b119187d97ad70539f1624bbede92648e2dc007890f9658a88756c5a06fb2e3d4ce2c438c0856c2de34948b7032b1aadc4642a9666228ea8cdc7786b7")[..]);

        let new_payload = ExecutionPayloadV3 {
            payload_inner: ExecutionPayloadV2 {
                payload_inner: ExecutionPayloadV1 {
                    base_fee_per_gas:  U256::from(7u64),
                    block_number: 0xa946u64,
                    block_hash: hex!("a5ddd3f286f429458a39cafc13ffe89295a7efa8eb363cf89a1a4887dbcf272b").into(),
                    logs_bloom: hex!("00200004000000000000000080000000000200000000000000000000000000000000200000000000000000000000000000000000800000000200000000000000000000000000000000000008000000200000000000000000000001000000000000000000000000000000800000000000000000000100000000000030000000000000000040000000000000000000000000000000000800080080404000000000000008000000000008200000000000200000000000000000000000000000000000000002000000000000000000000000000000000000001000000000000000000000000000000000000000000000000000000000000100000000000000000000").into(),
                    extra_data: hex!("d883010d03846765746888676f312e32312e31856c696e7578").into(),
                    gas_limit: 0x1c9c380,
                    gas_used: 0x1f4a9,
                    timestamp: 0x651f35b8,
                    fee_recipient: hex!("f97e180c050e5ab072211ad2c213eb5aee4df134").into(),
                    parent_hash: hex!("d829192799c73ef28a7332313b3c03af1f2d5da2c36f8ecfafe7a83a3bfb8d1e").into(),
                    prev_randao: hex!("753888cc4adfbeb9e24e01c84233f9d204f4a9e1273f0e29b43c4c148b2b8b7e").into(),
                    receipts_root: hex!("4cbc48e87389399a0ea0b382b1c46962c4b8e398014bf0cc610f9c672bee3155").into(),
                    state_root: hex!("017d7fa2b5adb480f5e05b2c95cb4186e12062eed893fc8822798eed134329d1").into(),
                    transactions: vec![first_transaction_raw, second_transaction_raw],
                },
                withdrawals: vec![],
            },
            blob_gas_used: 0xc0000,
            excess_blob_gas: 0x580000,
        };

        let mut block = try_payload_v3_to_block(new_payload.clone()).unwrap();

        // this newPayload came with a parent beacon block root, we need to manually insert it
        // before hashing
        let parent_beacon_block_root =
            hex!("531cd53b8e68deef0ea65edfa3cda927a846c307b0907657af34bc3f313b5871");
        block.header.parent_beacon_block_root = Some(parent_beacon_block_root.into());

        let converted_payload = block_to_payload_v3(block.seal_slow());

        // ensure the payloads are the same
        assert_eq!(new_payload, converted_payload);
    }

    #[test]
    fn payload_to_block_rejects_network_encoded_tx() {
        let first_transaction_raw = Bytes::from_static(&hex!("b9017e02f9017a8501a1f0ff438211cc85012a05f2008512a05f2000830249f094d5409474fd5a725eab2ac9a8b26ca6fb51af37ef80b901040cc7326300000000000000000000000000000000000000000000000000000000000000a000000000000000000000000000000000000000000000001bdd2ed4b616c800000000000000000000000000001e9ee781dd4b97bdef92e5d1785f73a1f931daa20000000000000000000000007a40026a3b9a41754a95eec8c92c6b99886f440c000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000020000000000000000000000009ae80eb647dd09968488fa1d7e412bf8558a0b7a0000000000000000000000000f9815537d361cb02befd9918c95c97d4d8a4a2bc001a0ba8f1928bb0efc3fcd01524a2039a9a2588fa567cd9a7cc18217e05c615e9d69a0544bfd11425ac7748e76b3795b57a5563e2b0eff47b5428744c62ff19ccfc305")[..]);
        let second_transaction_raw = Bytes::from_static(&hex!("b9013c03f901388501a1f0ff430c843b9aca00843b9aca0082520894e7249813d8ccf6fa95a2203f46a64166073d58878080c005f8c6a00195f6dff17753fc89b60eac6477026a805116962c9e412de8015c0484e661c1a001aae314061d4f5bbf158f15d9417a238f9589783f58762cd39d05966b3ba2fba0013f5be9b12e7da06f0dd11a7bdc4e0db8ef33832acc23b183bd0a2c1408a757a0019d9ac55ea1a615d92965e04d960cb3be7bff121a381424f1f22865bd582e09a001def04412e76df26fefe7b0ed5e10580918ae4f355b074c0cfe5d0259157869a0011c11a415db57e43db07aef0de9280b591d65ca0cce36c7002507f8191e5d4a80a0c89b59970b119187d97ad70539f1624bbede92648e2dc007890f9658a88756c5a06fb2e3d4ce2c438c0856c2de34948b7032b1aadc4642a9666228ea8cdc7786b7")[..]);

        let new_payload = ExecutionPayloadV3 {
            payload_inner: ExecutionPayloadV2 {
                payload_inner: ExecutionPayloadV1 {
                    base_fee_per_gas:  U256::from(7u64),
                    block_number: 0xa946u64,
                    block_hash: hex!("a5ddd3f286f429458a39cafc13ffe89295a7efa8eb363cf89a1a4887dbcf272b").into(),
                    logs_bloom: hex!("00200004000000000000000080000000000200000000000000000000000000000000200000000000000000000000000000000000800000000200000000000000000000000000000000000008000000200000000000000000000001000000000000000000000000000000800000000000000000000100000000000030000000000000000040000000000000000000000000000000000800080080404000000000000008000000000008200000000000200000000000000000000000000000000000000002000000000000000000000000000000000000001000000000000000000000000000000000000000000000000000000000000100000000000000000000").into(),
                    extra_data: hex!("d883010d03846765746888676f312e32312e31856c696e7578").into(),
                    gas_limit: 0x1c9c380,
                    gas_used: 0x1f4a9,
                    timestamp: 0x651f35b8,
                    fee_recipient: hex!("f97e180c050e5ab072211ad2c213eb5aee4df134").into(),
                    parent_hash: hex!("d829192799c73ef28a7332313b3c03af1f2d5da2c36f8ecfafe7a83a3bfb8d1e").into(),
                    prev_randao: hex!("753888cc4adfbeb9e24e01c84233f9d204f4a9e1273f0e29b43c4c148b2b8b7e").into(),
                    receipts_root: hex!("4cbc48e87389399a0ea0b382b1c46962c4b8e398014bf0cc610f9c672bee3155").into(),
                    state_root: hex!("017d7fa2b5adb480f5e05b2c95cb4186e12062eed893fc8822798eed134329d1").into(),
                    transactions: vec![first_transaction_raw, second_transaction_raw],
                },
                withdrawals: vec![],
            },
            blob_gas_used: 0xc0000,
            excess_blob_gas: 0x580000,
        };

        let _block = try_payload_v3_to_block(new_payload.clone())
            .expect_err("execution payload conversion requires typed txs without a rlp header");
    }

    #[test]
    fn devnet_invalid_block_hash_repro() {
        let deser_block = r#"
        {
            "parentHash": "0xae8315ee86002e6269a17dd1e9516a6cf13223e9d4544d0c32daff826fb31acc",
            "feeRecipient": "0xf97e180c050e5ab072211ad2c213eb5aee4df134",
            "stateRoot": "0x03787f1579efbaa4a8234e72465eb4e29ef7e62f61242d6454661932e1a282a1",
            "receiptsRoot": "0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421",
            "logsBloom": "0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000",
            "prevRandao": "0x918e86b497dc15de7d606457c36ca583e24d9b0a110a814de46e33d5bb824a66",
            "blockNumber": "0x6a784",
            "gasLimit": "0x1c9c380",
            "gasUsed": "0x0",
            "timestamp": "0x65bc1d60",
            "extraData": "0x9a726574682f76302e312e302d616c7068612e31362f6c696e7578",
            "baseFeePerGas": "0x8",
            "blobGasUsed": "0x0",
            "excessBlobGas": "0x0",
            "blockHash": "0x340c157eca9fd206b87c17f0ecbe8d411219de7188a0a240b635c88a96fe91c5",
            "transactions": [],
            "withdrawals": [
                {
                    "index": "0x5ab202",
                    "validatorIndex": "0xb1b",
                    "address": "0x388ea662ef2c223ec0b047d41bf3c0f362142ad5",
                    "amount": "0x19b3d"
                },
                {
                    "index": "0x5ab203",
                    "validatorIndex": "0xb1c",
                    "address": "0x388ea662ef2c223ec0b047d41bf3c0f362142ad5",
                    "amount": "0x15892"
                },
                {
                    "index": "0x5ab204",
                    "validatorIndex": "0xb1d",
                    "address": "0x388ea662ef2c223ec0b047d41bf3c0f362142ad5",
                    "amount": "0x19b3d"
                },
                {
                    "index": "0x5ab205",
                    "validatorIndex": "0xb1e",
                    "address": "0x388ea662ef2c223ec0b047d41bf3c0f362142ad5",
                    "amount": "0x19b3d"
                },
                {
                    "index": "0x5ab206",
                    "validatorIndex": "0xb1f",
                    "address": "0x388ea662ef2c223ec0b047d41bf3c0f362142ad5",
                    "amount": "0x19b3d"
                },
                {
                    "index": "0x5ab207",
                    "validatorIndex": "0xb20",
                    "address": "0x388ea662ef2c223ec0b047d41bf3c0f362142ad5",
                    "amount": "0x19b3d"
                },
                {
                    "index": "0x5ab208",
                    "validatorIndex": "0xb21",
                    "address": "0x388ea662ef2c223ec0b047d41bf3c0f362142ad5",
                    "amount": "0x15892"
                },
                {
                    "index": "0x5ab209",
                    "validatorIndex": "0xb22",
                    "address": "0x388ea662ef2c223ec0b047d41bf3c0f362142ad5",
                    "amount": "0x19b3d"
                },
                {
                    "index": "0x5ab20a",
                    "validatorIndex": "0xb23",
                    "address": "0x388ea662ef2c223ec0b047d41bf3c0f362142ad5",
                    "amount": "0x19b3d"
                },
                {
                    "index": "0x5ab20b",
                    "validatorIndex": "0xb24",
                    "address": "0x388ea662ef2c223ec0b047d41bf3c0f362142ad5",
                    "amount": "0x17db2"
                },
                {
                    "index": "0x5ab20c",
                    "validatorIndex": "0xb25",
                    "address": "0x388ea662ef2c223ec0b047d41bf3c0f362142ad5",
                    "amount": "0x19b3d"
                },
                {
                    "index": "0x5ab20d",
                    "validatorIndex": "0xb26",
                    "address": "0x388ea662ef2c223ec0b047d41bf3c0f362142ad5",
                    "amount": "0x19b3d"
                },
                {
                    "index": "0x5ab20e",
                    "validatorIndex": "0xa91",
                    "address": "0x388ea662ef2c223ec0b047d41bf3c0f362142ad5",
                    "amount": "0x15892"
                },
                {
                    "index": "0x5ab20f",
                    "validatorIndex": "0xa92",
                    "address": "0x388ea662ef2c223ec0b047d41bf3c0f362142ad5",
                    "amount": "0x1c05d"
                },
                {
                    "index": "0x5ab210",
                    "validatorIndex": "0xa93",
                    "address": "0x388ea662ef2c223ec0b047d41bf3c0f362142ad5",
                    "amount": "0x15892"
                },
                {
                    "index": "0x5ab211",
                    "validatorIndex": "0xa94",
                    "address": "0x388ea662ef2c223ec0b047d41bf3c0f362142ad5",
                    "amount": "0x19b3d"
                }
            ]
        }
        "#;

        // deserialize payload
        let payload: ExecutionPayload =
            serde_json::from_str::<ExecutionPayloadV3>(deser_block).unwrap().into();

        // NOTE: the actual block hash here is incorrect, it is a result of a bug, this was the
        // fix:
        // <https://github.com/paradigmxyz/reth/pull/6328>
        let block_hash_with_blob_fee_fields =
            b256!("a7cdd5f9e54147b53a15833a8c45dffccbaed534d7fdc23458f45102a4bf71b0");

        let versioned_hashes = vec![];
        let parent_beacon_block_root =
            b256!("1162de8a0f4d20d86b9ad6e0a2575ab60f00a433dc70d9318c8abc9041fddf54");

        // set up cancun payload fields
        let cancun_fields = CancunPayloadFields { parent_beacon_block_root, versioned_hashes };

        // convert into block
        let block = try_into_block(payload, Some(cancun_fields.parent_beacon_block_root)).unwrap();

        // Ensure the actual hash is calculated if we set the fields to what they should be
        validate_block_hash(block_hash_with_blob_fee_fields, block).unwrap();
    }
}
