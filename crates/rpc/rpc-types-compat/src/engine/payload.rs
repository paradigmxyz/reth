//! Standalone Conversion Functions for Handling Different Versions of Execution Payloads in
//! Ethereum's Engine

use reth_primitives::{
    constants::{EMPTY_OMMER_ROOT_HASH, MAXIMUM_EXTRA_DATA_SIZE, MIN_PROTOCOL_BASE_FEE_U256},
    proofs::{self},
    Block, Header, Request, SealedBlock, TransactionSigned, UintTryTo, Withdrawals, B256, U256,
};
use reth_rpc_types::engine::{
    payload::{ExecutionPayloadBodyV1, ExecutionPayloadFieldV2, ExecutionPayloadInputV2},
    ExecutionPayload, ExecutionPayloadV1, ExecutionPayloadV2, ExecutionPayloadV3,
    ExecutionPayloadV4, PayloadError,
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
        requests_root: None,
        extra_data: payload.extra_data,
        // Defaults
        ommers_hash: EMPTY_OMMER_ROOT_HASH,
        difficulty: Default::default(),
        nonce: Default::default(),
    };

    Ok(Block {
        header,
        body: transactions,
        ommers: Default::default(),
        withdrawals: None,
        requests: None,
    })
}

/// Converts [ExecutionPayloadV2] to [Block]
pub fn try_payload_v2_to_block(payload: ExecutionPayloadV2) -> Result<Block, PayloadError> {
    // this performs the same conversion as the underlying V1 payload, but calculates the
    // withdrawals root and adds withdrawals
    let mut base_sealed_block = try_payload_v1_to_block(payload.payload_inner)?;
    let withdrawals_root = proofs::calculate_withdrawals_root(&payload.withdrawals);
    base_sealed_block.withdrawals = Some(payload.withdrawals.into());
    base_sealed_block.header.withdrawals_root = Some(withdrawals_root);
    Ok(base_sealed_block)
}

/// Converts [ExecutionPayloadV3] to [Block]
///
/// This requires the [EIP-4788](https://eips.ethereum.org/EIPS/eip-4788) parent beacon block root
pub fn try_payload_v3_to_block(
    payload: ExecutionPayloadV3,
    parent_beacon_block_root: B256,
) -> Result<Block, PayloadError> {
    // this performs the same conversion as the underlying V2 payload, but inserts the blob gas
    // used and excess blob gas
    let mut base_block = try_payload_v2_to_block(payload.payload_inner)?;

    base_block.header.blob_gas_used = Some(payload.blob_gas_used);
    base_block.header.excess_blob_gas = Some(payload.excess_blob_gas);
    base_block.header.parent_beacon_block_root = Some(parent_beacon_block_root);

    Ok(base_block)
}

/// Converts [ExecutionPayloadV4] to [Block]
///
/// This requires the [EIP-4788](https://eips.ethereum.org/EIPS/eip-4788) parent beacon block root
pub fn try_payload_v4_to_block(
    payload: ExecutionPayloadV4,
    parent_beacon_block_root: B256,
) -> Result<Block, PayloadError> {
    let ExecutionPayloadV4 { payload_inner, deposit_requests, withdrawal_requests } = payload;
    let mut block = try_payload_v3_to_block(payload_inner, parent_beacon_block_root)?;

    // attach requests with asc type identifiers
    let requests = deposit_requests
        .into_iter()
        .map(Request::DepositRequest)
        .chain(withdrawal_requests.into_iter().map(Request::WithdrawalRequest))
        .collect::<Vec<_>>();

    let requests_root = proofs::calculate_requests_root(&requests);
    block.header.requests_root = Some(requests_root);
    block.requests = Some(requests.into());

    Ok(block)
}

/// Converts [SealedBlock] to [ExecutionPayload]
pub fn block_to_payload(value: SealedBlock) -> ExecutionPayload {
    if value.header.requests_root.is_some() {
        ExecutionPayload::V4(block_to_payload_v4(value))
    } else if value.header.parent_beacon_block_root.is_some() {
        // block with parent beacon block root: V3
        ExecutionPayload::V3(block_to_payload_v3(value))
    } else if value.withdrawals.is_some() {
        // block with withdrawals: V2
        ExecutionPayload::V2(block_to_payload_v2(value))
    } else {
        // otherwise V1
        ExecutionPayload::V1(block_to_payload_v1(value))
    }
}

/// Converts [SealedBlock] to [ExecutionPayloadV1]
pub fn block_to_payload_v1(value: SealedBlock) -> ExecutionPayloadV1 {
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
pub fn block_to_payload_v2(value: SealedBlock) -> ExecutionPayloadV2 {
    let transactions = value.raw_transactions();

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
        withdrawals: value.withdrawals.unwrap_or_default().into_inner(),
    }
}

/// Converts [SealedBlock] to [ExecutionPayloadV3]
pub fn block_to_payload_v3(value: SealedBlock) -> ExecutionPayloadV3 {
    let transactions = value.raw_transactions();

    ExecutionPayloadV3 {
        blob_gas_used: value.blob_gas_used.unwrap_or_default(),
        excess_blob_gas: value.excess_blob_gas.unwrap_or_default(),
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
            withdrawals: value.withdrawals.unwrap_or_default().into_inner(),
        },
    }
}

/// Converts [SealedBlock] to [ExecutionPayloadV4]
pub fn block_to_payload_v4(mut value: SealedBlock) -> ExecutionPayloadV4 {
    let (deposit_requests, withdrawal_requests) =
        value.requests.take().unwrap_or_default().into_iter().fold(
            (Vec::new(), Vec::new()),
            |(mut deposits, mut withdrawals), request| {
                match request {
                    Request::DepositRequest(r) => {
                        deposits.push(r);
                    }
                    Request::WithdrawalRequest(r) => {
                        withdrawals.push(r);
                    }
                    _ => {}
                };

                (deposits, withdrawals)
            },
        );

    ExecutionPayloadV4 {
        deposit_requests,
        withdrawal_requests,
        payload_inner: block_to_payload_v3(value),
    }
}

/// Converts [SealedBlock] to [ExecutionPayloadFieldV2]
pub fn convert_block_to_payload_field_v2(value: SealedBlock) -> ExecutionPayloadFieldV2 {
    // if there are withdrawals, return V2
    if value.withdrawals.is_some() {
        ExecutionPayloadFieldV2::V2(block_to_payload_v2(value))
    } else {
        ExecutionPayloadFieldV2::V1(block_to_payload_v1(value))
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
    ExecutionPayloadInputV2 {
        withdrawals: value.withdrawals.clone().map(Withdrawals::into_inner),
        execution_payload: block_to_payload_v1(value),
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
    let base_payload = match value {
        ExecutionPayload::V1(payload) => try_payload_v1_to_block(payload)?,
        ExecutionPayload::V2(payload) => try_payload_v2_to_block(payload)?,
        ExecutionPayload::V3(payload) => try_payload_v3_to_block(
            payload,
            parent_beacon_block_root.ok_or_else(|| PayloadError::PostCancunWithoutCancunFields)?,
        )?,
        ExecutionPayload::V4(payload) => try_payload_v4_to_block(
            payload,
            parent_beacon_block_root.ok_or_else(|| PayloadError::PostCancunWithoutCancunFields)?,
        )?,
    };

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

/// Converts [Block] to [ExecutionPayloadBodyV1]
pub fn convert_to_payload_body_v1(value: Block) -> ExecutionPayloadBodyV1 {
    let transactions = value.body.into_iter().map(|tx| {
        let mut out = Vec::new();
        tx.encode_enveloped(&mut out);
        out.into()
    });
    ExecutionPayloadBodyV1 {
        transactions: transactions.collect(),
        withdrawals: value.withdrawals.map(Withdrawals::into_inner),
    }
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
        block_to_payload_v3, try_into_block, try_payload_v3_to_block, try_payload_v4_to_block,
        validate_block_hash,
    };
    use reth_primitives::{b256, hex, Bytes, B256, U256};
    use reth_rpc_types::{
        engine::{CancunPayloadFields, ExecutionPayloadV3, ExecutionPayloadV4},
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

        // this newPayload came with a parent beacon block root, we need to manually insert it
        // before hashing
        let parent_beacon_block_root =
            b256!("531cd53b8e68deef0ea65edfa3cda927a846c307b0907657af34bc3f313b5871");

        let block = try_payload_v3_to_block(new_payload.clone(), parent_beacon_block_root).unwrap();

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

        let _block = try_payload_v3_to_block(new_payload, B256::random())
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

    #[test]
    fn parse_payload_v4() {
        let s = r#"{
      "baseFeePerGas": "0x2ada43",
      "blobGasUsed": "0x0",
      "blockHash": "0xedfe4ef7d8a7c116f68f7e72bdb75be157bedf03ac8b63fa8d5762495b0473d3",
      "blockNumber": "0x2c",
      "depositRequests": [
        {
          "amount": "0xe8d4a51000",
          "index": "0x0",
          "pubkey": "0xaab5f2b3aad5c2075faf0c1d8937c7de51a53b765a21b4173eb2975878cea05d9ed3428b77f16a981716aa32af74c464",
          "signature": "0xa889cd238be2dae44f2a3c24c04d686c548f6f82eb44d4604e1bc455b6960efb72b117e878068a8f2cfb91ad84b7ebce05b9254207aa51a1e8a3383d75b5a5bd2439f707636ea5b17b2b594b989c93b000b33e5dff6e4bed9d53a6d2d6889b0c",
          "withdrawalCredentials": "0x00ab9364f8bf7561862ea0fc3b69c424c94ace406c4dc36ddfbf8a9d72051c80"
        },
        {
          "amount": "0xe8d4a51000",
          "index": "0x1",
          "pubkey": "0xb0b1b3b51cf688ead965a954c5cc206ba4e76f3f8efac60656ae708a9aad63487a2ca1fb30ccaf2ebe1028a2b2886b1b",
          "signature": "0xb9759766e9bb191b1c457ae1da6cdf71a23fb9d8bc9f845eaa49ee4af280b3b9720ac4d81e64b1b50a65db7b8b4e76f1176a12e19d293d75574600e99fbdfecc1ab48edaeeffb3226cd47691d24473821dad0c6ff3973f03e4aa89f418933a56",
          "withdrawalCredentials": "0x002d2b75f4a27f78e585a4735a40ab2437eceb12ec39938a94dc785a54d62513"
        }
      ],
      "excessBlobGas": "0x0",
      "extraData": "0x726574682f76302e322e302d626574612e372f6c696e7578",
      "feeRecipient": "0x8943545177806ed17b9f23f0a21ee5948ecaa776",
      "gasLimit": "0x1855e85",
      "gasUsed": "0x25f98",
      "logsBloom": "0x10000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000400000000000000000000000020000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008000000000000000000000000",
      "parentHash": "0xd753194ef19b5c566b7eca6e9ebcca03895b548e1e93a20a23d922ba0bc210d4",
      "prevRandao": "0x8c52256fd491776dc32f531ad4c0dc1444684741bca15f54c9cd40c60142df90",
      "receiptsRoot": "0x510e7fb94279897e5dcd6c1795f6137d8fe02e49e871bfea7999fd21a89f66aa",
      "stateRoot": "0x59ae0706a2b47162666fc7af3e30ff7aa34154954b68cc6aed58c3af3d58c9c2",
      "timestamp": "0x6643c5a9",
      "transactions": [
        "0x02f9021e8330182480843b9aca0085174876e80083030d40944242424242424242424242424242424242424242893635c9adc5dea00000b901a422895118000000000000000000000000000000000000000000000000000000000000008000000000000000000000000000000000000000000000000000000000000000e0000000000000000000000000000000000000000000000000000000000000012049f42823819771c6bbbd9cb6649850083fd3b6e5d0beb1069342c32d65a3b0990000000000000000000000000000000000000000000000000000000000000030aab5f2b3aad5c2075faf0c1d8937c7de51a53b765a21b4173eb2975878cea05d9ed3428b77f16a981716aa32af74c46400000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002000ab9364f8bf7561862ea0fc3b69c424c94ace406c4dc36ddfbf8a9d72051c800000000000000000000000000000000000000000000000000000000000000060a889cd238be2dae44f2a3c24c04d686c548f6f82eb44d4604e1bc455b6960efb72b117e878068a8f2cfb91ad84b7ebce05b9254207aa51a1e8a3383d75b5a5bd2439f707636ea5b17b2b594b989c93b000b33e5dff6e4bed9d53a6d2d6889b0cc080a0db786f0d89923949e533680524f003cebd66f32fbd30429a6b6bfbd3258dcf60a05241c54e05574765f7ddc1a742ae06b044edfe02bffb202bf172be97397eeca9",
        "0x02f9021e8330182401843b9aca0085174876e80083030d40944242424242424242424242424242424242424242893635c9adc5dea00000b901a422895118000000000000000000000000000000000000000000000000000000000000008000000000000000000000000000000000000000000000000000000000000000e00000000000000000000000000000000000000000000000000000000000000120d694d6a0b0103651aafd87db6c88297175d7317c6e6da53ccf706c3c991c91fd0000000000000000000000000000000000000000000000000000000000000030b0b1b3b51cf688ead965a954c5cc206ba4e76f3f8efac60656ae708a9aad63487a2ca1fb30ccaf2ebe1028a2b2886b1b000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000020002d2b75f4a27f78e585a4735a40ab2437eceb12ec39938a94dc785a54d625130000000000000000000000000000000000000000000000000000000000000060b9759766e9bb191b1c457ae1da6cdf71a23fb9d8bc9f845eaa49ee4af280b3b9720ac4d81e64b1b50a65db7b8b4e76f1176a12e19d293d75574600e99fbdfecc1ab48edaeeffb3226cd47691d24473821dad0c6ff3973f03e4aa89f418933a56c080a099dc5b94a51e9b91a6425b1fed9792863006496ab71a4178524819d7db0c5e88a0119748e62700234079d91ae80f4676f9e0f71b260e9b46ef9b4aff331d3c2318"
      ],
      "withdrawalRequests": [],
      "withdrawals": []
    }"#;

        let parent_beacon_block =
            b256!("d9851db05fa63593f75e2b12c4bba9f47740613ca57da3b523a381b8c27f3297");

        let payload = serde_json::from_str::<ExecutionPayloadV4>(s).unwrap();
        let block = try_payload_v4_to_block(payload, parent_beacon_block).unwrap().seal_slow();
        let _hash = block.hash();
    }

    #[test]
    fn devnet() {
        let payload = r#"{"baseFeePerGas":"0x113d70b","blobGasUsed":"0x0","blockHash":"0x908a3ba879bc36d4b34244759c9d665ee0f3e6baa2604eb9f99fac73183b04af","blockNumber":"0x23","depositRequests":[{"amount":"0xe8d4a51000","index":"0x0","pubkey":"0x8781f734dcbbb41234a2f6e5b08b6dcd5babec3635011f6eeafc58c6e154d4d07549aaf5cc7fa05f127dc485bd757b49","signature":"0xb47afffc53dcf130939ed6d516cb29f9d9a4631e5d122a0b3b86cdceca6ffe91acad77a02eb78556a1295fca7e08e8630d5e08433f72847f78b62a3913cc83f54c3486650e9d440db7cdbd836ac0b840db814a03cccfd3759e2ea55f929d999f","withdrawalCredentials":"0x00e02267460dfed7870106fe6be1131e22ad802fecab489e13d4295980f696e9"}],"excessBlobGas":"0x0","extraData":"0x726574682f76302e322e302d626574612e372f6c696e7578","feeRecipient":"0x8943545177806ed17b9f23f0a21ee5948ecaa776","gasLimit":"0x17e969d","gasUsed":"0x6fdbea","logsBloom":"0x10000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000400000000000000000000000020000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008000000000000000000000000","parentHash":"0x6c0a907b61b1ebbc450b9cd1ea97f1dada5ca46a73642c80b7de48a2caad824c","prevRandao":"0x88b57279222bbaf9710e1978916a6cebb7835d9b36c6f0aabcf6281fd244d9e7","receiptsRoot":"0xbafebee73190ad29e37143073e51f6dccc53ce23171ea5aa501996035af02773","stateRoot":"0x29dcb3f89224e449d55ab152df6855bbbc36b373f5c09348287606de4a935b7d","timestamp":"0x6643bd0a","transactions":["0xf8d4098456bbcb5c830186a08080b8804a3d6000593e5960002080557fd0dacf728f8698c6a3816207a84551a375c9401a21f4cc2dddcd46e8473dbc8e603c527fbf5000465de1211b0b48719ff8cca04a45a3ec858b4e6a1b3ab58ef60f52913b605c527ff002d7a36a6d65236d2d339bb865b62bebe6d0082a7d628c97287a8ddd706e4a607c527fcf702ce56a99d18360306ba080a94086b7f235cf5584036314b78038f0e43b3cf6a019a92137301e36481059a06d4be485bcb2c282e19102e34a513031272afe397d639c5c394f084ada67f4ca","0x01f8ea83301824098456bbcb5c830186a094ec532bab3afa72da94306ba7f5c8184a884080bc80b88060006055554f51e7337f31eb71d9a6b3ffbded44b119c72059aec525195af2342a4a04cb604ff62588b2605e527f95a01aaac64f31406ec75284223ef9264f1dabd8b243943938b8e49af28ecee3607e52606f609e536041609f5360f560a05360bc60a15360ab60a25360d160a353603260a45360c660a553604060a65360e4c001a084854536a1ed1d76b2e35ff6ec488367f975de18b893d09d7a43c5fb3a9402cea0214deb9d9f5f84bd8eae4a19995261020ab361079df33c274e7c49774a40c341","0x01f8ea83301824068456bbcb5c830186a094000000000000000000000000000000000000000080b8807ff5d862f4f24a49e74178352f9001b92045408ff958a17efdd88c6830ff1f557b6008527f51ba96bda099d134f73138bca9927a400989c229f2205cb3c2ab0d8fe1ae87ca6028527fe4483ccb89526b9854ac17a32a33c8f09ba7adc32964cfc258db652fa9f1312f6048527f4b64239ad6e309374f6c037c0c91c94643ce35c001a08f432540e0c3c05596ce8017228a90eb5fe77039b739ecc5c9ede69222f17de7a0750c3faf62de7e99ddb792346b321b9d6a227a2e5f7f04f3d041d0837f346d0b","0xf8d4048456bbcb5c830186a08080b88060006002556000608e557fd1fac11f9bf60ee076b7a24e0b573bdd184048500d7964d8343b29e8840ddcc360c8527f43e9024dfc16200d4cf4b884bed910ab7dbec017671b9058191ba4ab52b0c2f560e8527f3e5bdfadf9dd1d332463e1d6147170eeac58d24ee010da9354a100b5aa62edb7610108527f48a0bc30f79528b18360306ca0543a96fad769bf67e8f4bab6df1af1bb320629cadb43905cff22eec614bd343ea06341a3b564bce959242e2ddcecc7447f259c5a3a7940a0acb1e5ff61f04da974","0xf8d40c8456bbcb5c830186a08080b88060e860345360206035536094603653608360375360ef6038536020603953605c603a53fe60006061556000607b55f46000608755b16000605455600060e5553b7ffd012f1b132047c281cba1811212feceaa0919bcf70106ffac1eca3ebce66cfd60b2527ff3df5c2021c483eadc047408749258281b4ca395d7a85d8461a3068360306ba0d886f9457b5330570bf3d1066b339bb718e35b4340bbc0a3c150c7a088a71e19a02136466db9f582bb5850f2a17f42ccab34186b08bb4bb026c903731ef4e1874d","0xf8d4098456bbcb5c830186a08080b880f0643f1d7fbeac7e39c03da75cf722ea703c1c2b29d79727528da6f7bce80dc658a0c9d6d4608c527f8de7e8da9f821bd6499a5b7a6f0ee8a2f305e465373d36e64cf87c8faca14c0760ac52608b60cc53608660cd5360b260ce53609860cf5360cb60d053608960d153603660d253603060d353609060d45360c960d55360158360306ca058895543cdde16142ddcf567566cfb12c52bcba687e42d17e4e3516f5ccb5682a01ef77b3d7b3aa8b69d92b908cfdac7feeb37f59d5c33b69cbf2724eeb7bd8ea0","0xf8e8078456bbcb5c830186a094000000000000000000000000000000000000000080b880aab47f3e5e9dfbfc638c05ff6e8559e11c6ce0c8804a4d51d8cab3625d006d2444c2c2604d527f1d0f9ce39ca171bd3a70579056f709f53ed701a20ff52b82d2b2e41355219d47606d5260d7608d5360cd608e53600a608f53604960905360e260915360ab6092536072609353604b609453606f6095536089609653606460978360306ba03b783e926ac7fa2c6d38df54958ca459fdf888c20e86a70042656c3dc1db5334a0174c398ddc031e16ecbdabff5ae4fbf634b5a04644939a0609e7d6d3f257525b","0x01f8d683301824078456bbcb5c830186a08080b880600060d555600060c2557fb851f91d5fce317f6add1dff4da61e1ee9e5500666da66ad1afd7bfb1e6f54456069527f1850d494cd97a3883e23c661b828efd5048d13f713806dfceafb8f78b1aae62f6089527f280848483d70b588e71ecd4a1b7da39ae4bee2729bfb0ee6b55dc62a3095b0ca60a9527f411ec2e022ec6dbacbc001a0b66a9896b2dd78ddad0f70f82e836c4e5d4e030531681f9ca7b9f87014b8eb34a020b3cf2309307c082a55e43d346c5bf540da56ed6ac3b8ccdf6f335aee2a6ff2","0xf8e80a8456bbcb5c830186a09496fad845ef6261bbd22b7cafe48aa631c663af0d80b880600060e65560006096556000602f5560b6604d5360aa604e536083604f5360a0605053605d6051536f9d600060d5550e600060e455604760f553607a60f653608260f753608060f853602060f95360b860fa53602360fb53600f60fc53602660fd5360a760fe53607c60ff5360366101005360256101015360266101025360828360306ca03cc38857da7c99ea0efee47d81ef4a66004ab5b85e5acf4367a9daf4b450acd0a01c9bd78cca8bbc160163ccae86a7a638c28d3e922f652c9d01fa497855395d3c","0xf8d4098456bbcb5c830186a08080b8802a39600060da55bd7fd8be47f7f78ed2a28fe20bb398dbb3a3db7a5ab5c4de0929eb774cc1e1e6280760a0527fb22bd3038b7f85bb8a3cef4fd67760d1ebd6e85441785904e61dfe5c9a3614d560c0527fd95daa72ddcbb4619b8f45af2530b581b4a52c695ec724c4db34faaac060699660e0527f9fec3c73ae95120ed061e98360306ca05bf8731f1199a76f36e3ac7b63f7faf00577722a285749c100fe5f1fc0aa59dfa0619e723a3ad27c25f2ccd907ec101232d1b0f176d66837482983efd9ea8c5ade","0xf8e8088456bbcb5c830186a094000000000000000000000000000000000000000080b8807fff87eb0b568ce9c4814c04f04ac2ba456580bc93ad8f65ddbd571fb4e8accb2760a8527fa95d949930c4e87ee15f6d9b107fc2a28273ccd6a2063b7c69dc9f4bda18e85960c85260a960e853603060e95360b360ea53609260eb53602260ec5360b860ed53609f60ee5360f760ef53606c60f053601560f153603760f253608360306ca0b98af740ac6869a924539d30d2df98ba53fa1ef314f0ed280208c3834ccd3d27a02d8fb3484a599e5b8111c1a85a86f36837cc45852eb4de00a9f40d8cf71089f8","0xf8e80a8456bbcb5c830186a094052bbaa1cc63fddba322cb90457d9d14649732f780b8807f26d385650766f19c69143eb4efa6a5ddb95f161fc1df6a98dc1a380864be684a6080527fcfcca4ee7b00a68a1f078f9c9d02a87198686533559b0d0136211972cb54df4060a0527f25a329fcad698e9274f60bfb4d37199419d579589cd0fe7e9b4ca4cfd5a044bc60c0527fa8224a3e5e226316d98ecce401c80c872c27e78360306ca0e7f2fdd68ecf2c9ca5b226cb24b09bee118b89b166db1c50c39517df0eceb38ba01d55963346d024a512d7caea2fd61a10078b26f7809cbf2449ce0a6073ea5663","0x01f8d6833018240b8456bbcb5c830186a08080b880b75f2a7fb5c8c79ef90b4117acfdba0da5ddb3fb2eef5f94bf43350aef2f7f6c712b5a1b606f527f499c9bb66217c6ceaca83f719920c524759f5d8a7cfd700b8ce79c5854eb09da608f52608360af53609c60b053602460b153608860b25360f260b353604360b453606760b55360bc60b65360db60b75360c660b853608560c080a0d4e1826979bae8b2a4e3d92772fcf32a2123ec5472f18f8d1e43e14c7c6ec60fa01ab93d8b903571a359efd2f617e1443032d6b41faa3889c8b5c1dd75ac40825d","0xf8e80b8456bbcb5c830186a09438b147387d23ca31b3afa29bfe74515d43b47c2c80b8806000605155686000600e5560006023558b7fb8233cc72118b9d1a8ab9764d0362158f1eb4121c444bc0fe7cda2fdbfc0485760f0527f9f8a0d86ee172f53260557ea4ac3e51800d303ebbd0573cec5462855b687df3b610110527ff29feefeddda5abe45e07089efe58c832acc0e0f7c423376b411d0ba02532a1c61013052608360306ca0906cb63dbecce0a6f21776d652513d63a5b64ff6e739f86ec948cdab97db9f57a0124c01ac84bb4bd5dea9a2c72d04544df8fd5a3198175329567ea5ecf7c7fc04","0xf8d40b8456bbcb5c830186a08080b880d96000604755466000604f55cc7f90e20b254f03e882b88a6b393c2fe709159763f26666479eb626daaec8efaa6b605b527f11f0a9606b6e50dd7ba3e504a1b7735774581784872e5b1dea295b3c0bc7828b607b52606b609b536012609c5360a9609d53602c609e536090609f53602e60a05360f360a153600360a2536045608360306ca07715fa3dba76ea10d53daeff82356f043de8159890dfa66e5c8d0d9ef3a4bb92a06419ca21d0e31da6ea7ebf033b12413ee5b86bf1171b350d900201fbab4d6afd","0x01f8ea83301824098456bbcb5c830186a094b02a2eda1b317fbd16760128836b0ac59b560e9d80b8807f46b7cb78196e3aa1568ff4eb8c902919a12229e9268e597ee55c497719e3ff376034527f6aa97c6499ac0a881c376c2922450433a6b923615eed2a39167e220bcd4d50166054527f0f36f8c1899fdc2e3538fc9939e5911e3458c3103659532e846a2cf77391f0ec6074527f31ba8fe8f9fc29b95ebacc711f28c1689ccf72c001a044ec2e348c408a73b93925468defb12ce4d65bc6c9b0cae0172c3690dbe6deafa0749e4b5d8cb9159ec132d0ce6b2b425161e9cd2750ba091a2e401c62ec4143af","0x01f8ea83301824088456bbcb5c830186a094ea8098e52cf4eefce376101f2164cbcd44e1847880b8807fe441572358a471378730a6375d31cf79f8bc80e9b2e4a9827fb11942b4bf28006035527f8f8c24e162ff2074b0bcdc56bb177a51cd46782712dd36ef764adeada99907f16055527fc1905d77a574590cce5340a6d1ebafc27d674f3a6c07c16b0c91f9731b4144326075527f2aa1fb5736d4d1e469be4820fe5f949c849d6fc001a0768d58b2e4aaab5bced2163f0d9a5f608df2bc28b677ea05082ec149273de35ca01043420dbb37a40db558e4697d4b84e23546496fbb16fd9bdadcab5460f91de7","0x01f8ea83301824088456bbcb5c830186a094000000000000000000000000000000000000000080b880667fd2e565470731e66ab1b6785e652a5c4de556d7b899b5bfa4ab724182e275075260e5527fc787bcd7989ac74a393fb3ba71b1c4b68321b8726b8d4ae1e100082c37406a89610105527f4a235a3b6daa45f8ffcf5e09e44bfb08a4fb46ac841de73167c7566bc47925e46101255260f961014553600e610146536011610147c080a01e6c088f50e8682052cb4793c21d3b091fc97824273a86bf314d9f09f4365f67a069e3dee5607412f6103b3f25fa4bfad567500e82426bdc4f55d2e4b994462ba0","0x01f86e83301824058456bbcb5c830186a094395f3c99e78dd57733272cd15c908d95da28634d80856000604d55c001a04b4fea8eaffc8fde3fa213c978c50b1794e2c874984b45bd2df905281e25af3fa02f688bb83271c817b0929c44496ca67153376bc8312a0605c360615477d2cfc9","0x01f8d683301824078456bbcb5c830186a08080b8800d087f42c6068783700984da570fe2ec36ed560ba70c355c625a1105538d7a0630f41b6042527fb7bd03b457eec946a00139cd7b6964818dc894944ecabb82fa65d8beb3a3647c606252600960825360736083536053608453608060855360fc60865360486087536032608853602c6089536027608a5360c4608b5360ad608cc080a05cb09238dad28077be81a8aee99ed218a97196e10d13f5fcfa5a134adab47978a02e5975952b68405a75e2914b7337909a54427af00bb22410478844cf1bbda54d","0x01f8ea83301824088456bbcb5c830186a094b02a2eda1b317fbd16760128836b0ac59b560e9d80b8806000606a557f6ab899432fe5c59235bbeebfb94313b4c1706f7de0c5f8ceaa4a6e58d9d9d4c060da527f1209faf34a2787d4f2803f43ce17df9688c6ea0acc2a0ca3460d5ab0b640354760fa527f6ab37c038c781b528c4b0f25c9638da5684b2eae2e658ae53ec151abac616e6f61011a527fb00240b00d497e600dac5f6995c080a0bbbf6088a36c862312cea858fa7ad6774c174ae82a52a7dd1f962761acdc9093a049b0f15d68ddae7bacc4fa376e4cc09e6d484f9ed87db63067e8e89f90fcaa25","0x01f8ea833018240a8456bbcb5c830186a094b02a2eda1b317fbd16760128836b0ac59b560e9d80b88088600060eb55600060cb557ff52c1382ab27456d83750eaa45c4ae2366c8ae9b3a5394c42be483f750d1fdb46089527f755a048d09d9e4e659e08c129c8e3a7f28cbc257b56511598962279cd0e78bf860a9527fbfad8e141310a4731a467cc383c59601cd171d0c886db6aa7b1355cdce10c71b60c9527f57cfb1e9f53caba4c080a05c333f78830bbd546a6662c9495fc9260c9533dd02becfeaee131bd908f2df8ca05644da3cf43fb275e8ba7922a34e42b9cec12be18f7bec15e4b2162beaffd1fd","0xf8d4098456bbcb5c830186a08080b8807fe29c2000f2a68645aff8d8050d67bf2383ba7095ff8724b859dcd66eaac1dfb9601a527fa2eef8d9d70730f67506ad564c37d7d9531314927ed2a52c5b958a8cab4acf0f603a527f37aa8f5583aa3e00de178e0e7245a78a3b934306059469cf37e67154f9065e62605a5260f2607a5360d2607b536083607c536016607d538360306ca0690e96bb83777b572395b66133a4f7d335bdac7dbf40607a3e7f646ab345eabba0101e0ca840e786ac85f6c6df9b99a94d291e48f39e375e5a9b8d2c240dbb987c","0x01f8ea833018240b8456bbcb5c830186a094b02a2eda1b317fbd16760128836b0ac59b560e9d80b8800c35024660006097557f84203600de84e7e9ddb71a64bf7bb9ee53ca4feccbae5330cacd08952718d7396058527fcd948ec0d3dd0ccf7a80416a514590cdb2bad263e8a6f20989223c62bcbcafc760785260ff609853601660995360ca609a5360bf609b5360fc609c5360c0609d536092609e53600f609f53606960a05360f1c080a03f03a79073a50ca958a9f83a245be3a7788666517267dd9944e5115a6dc6a7d6a062c229ed4adcc44e35f9bd6b6556c13dd02df67c559b924bd2ed95c6c8caee4b","0x01f8ea833018240c8456bbcb5c830186a094dfa521defd2b49f775d1a7907c1d4fa89549128080b880e6608260ed53600760ee53600460ef53609960f053603860f153604260f253609360f353600860f453608660f55360cf60f653601660f753608f60f85360cd60f953600260fa53602160fb5360b060fc5360df60fd53607a60fe536000605a55937f552f753eb3083372a7beb193ac97590275f955e0ce35bf653d0585cbc77fc080a09028ae5577208bdc15bd049fc871cb18e316015a2f6de71a309b2066e5278b58a0174663240d9052a2dc09d7b70ec4968842ac7d79c9d283cd14b3649fd25ff33e","0xf8e8078456bbcb5c830186a0949595885f5202a5ad698912c94adedc3c1459581880b8806049601253607160135360a160145360db6015537fb08e8f908720060257d518b78b6ffde065c7f0151aae6951a05d6736f6d05ab2603a527fc0544c2677429ce6913e65a4e09c055ee237ec3a93dc9101285d5e763fda89b1605a527fd196a6f2b5153e0d439698901f84556c6a4f2294ec7f5ef0e7821e67dec80d34607a528360306ca06948eea176ce389c5fd794492268c23ea019a0913ab37e9a8d65f8a5ed071396a0118dd2142afac80e84983f5b51556f48a01121fc0c89300891ed627c4a69efc7","0x01f8d683301824088456bbcb5c830186a08080b880600060335560ce60ef5360de60f05360f360f153602460f253607160f353600960f453600460f553602460f653601760f753608f60f85360e160f953601160fa53601e60fb53606060fc53605060fd5360b960fe53609360ff53307b728c6000602a55d149df600060b3550b602460295360ac602a536027602b536013602c53c080a0f0aa70f793bcbab26d05b7d5394c6872c51e26bb696e1649d4194ec3222e485ca03216cef3cf955b104b61b415968ca5230aa5957cbb8619dd7959b31f6ceb6925","0x01f8ea833018240c8456bbcb5c830186a0948f44971fdd03e4622a0b1382e2434aec1b77acd380b8807fcc1c30707faa1c17e29e1b025a48f2bb876c9190333da8f27b2b3bf49d370615609c527f1400a964371c172aeaf7330588c01dc69aabaf5f9d0669edb453ee1b249ed79260bc527faa1afe49df8acd4ea69308f7bb795545913a9e2af6945043227c6af7ed762c9060dc527facc3813cece896070dd9bea7a1eec2a9217d01c080a0b94f14ce3b00820e8c7b4cd6842a12cfb0288d21165bafee6a86103c3336568da031bfb8f92858ca2b0831b4f3c7bc98c3b14cbb8959c455a0ce3fc7963932dfd2","0x01f9010f833018240a8456bbcb5c830186a08080b8807f18b5c80569903c0eb3afd5466142f0340de949d4f4472449d5b482830ebc8fd8609b527f928c1910d110ff017ad5b6ef5bca371b50c3d078773c3628b5910100920316e960bb5260e060db5330600060bd557f532771a235926874e3417b18d32b3606b08f09c91734d62ae8e3965660943792605b527f9772a32b694d36edf838f794f98b1d85ca267963a39deeef27716ac13c246878e1a000000000000000000000000000000000000000000000000000000000000000bd80a0484c6a91ca648016c8dd2aaf11f2d7e51f3b18e1955aa389ec1c639d9e80b89da03bef395129dea9ce1a68910efc4d99841ae5c961ae10c6f945e9776eb919482a","0x01f8d6833018240b8456bbcb5c830186a08080b880600060e3556d600060445545b0600060d6556000608655fa916000603f556000604d557fcb975dadb8e62b5786de6b132b3074eacefb6cff0257aa312ec05646e66f3e7f6032527f4c5f2013f2387122d5a5a9735a6d336f7a6ddc4e6fead9aae9085369560960846052527f4ef5419bda02cad3ffcafdb4593505681e092de0c001a02eaa6d29fdf870e258503e5aa49cef10a36574a79c843be27c4092c8bd05f46ca02a366a29f644ff6e5bcb5c7d4c4cb23eb8fb641a133d234dceda73367f4ea6a7","0x01f8d683301824098456bbcb5c830186a08080b880c860006097556000600155600060ed55600960ba5360d360bb5360b860bc53601460bd53605260be5360f460bf53601460c05360bd60c15360f260c253600060d655600060a6559394fa14c27776600060ea5560006020557fe0162fb3d772f4ddd606d1d2e4fdacc2d08417cc4b10c20243334c6fca5a963a6028527fcb8650c080a0f28a685723be3d941ae33463b5966330bfcf0cfd1ac3e64745d42a069c78e5cfa03cc6016181cc472798880c0e67b7b057e0cca92f4c0d0aa813543f494109116e","0x01f8ea83301824098456bbcb5c830186a0942ca50d9cc7149846cb4b33344516a98b0373cf3d80b880a07f03530483c476ac70da6f978b34e4cf4c8d4e8ffae0329713c847bdb32fb8f467608b52601060ab53607560ac5360c460ad53608560ae53807f83d729ba75a223d2d93be3df6cac2d9b35c87de8f8b5ec906f698a14a898709960e3527f8b9d88181e2b11be19f937846b4ad5378919323f6a7b42dc73240d317b47a47261c080a097af752c6fd758b58b12fb12f3f2c906a5f30a457dc8794d93c0d7bc3cf3ebc7a073273f87a1e7cba840c80a6351e21c094d78ccb72ebac0713ed764080e975f3e","0xf8d40a8456bbcb5c830186a08080b8807f4896eb14a4b173ff3c225c4b3f9552a009b590d7c2c502450cc53c931398f7d360c2527fb33809338fda85c01816ea80e5a5088e4368a577515e133bf89c0c8b8988aa8860e252608b6101025360976101035360db6101045360106101055360e56101065360df6101075360ab6101085360fd61010953607761010a53602b8360306ba0d71c820a8312e821279d9f2aedc43be9bff418337ca29a747464a2dbc0c0fe7da03d231757e314a1fd1dbdf1416e5a4db0af810e271ead735a5f490d2a5395b849","0x01f9010f83301824078456bbcb5c830186a08080b8806000603c557fdbf2999b81dee035438d0739b6d7d943ad246197703d9920edf07192be828959604d527fc72372754a2c48d11ade813903bc538a86355d29b73e3cfa6264e1d322d59027606d527fe098e3e949118d1ebc767e03add88118b6998a1c0e7b3d2b7bcce6d7c395a7a5608d527f2a29e9d4b7c0fdff58e4464b2a13f838f79428a0ce2f10d312780fff6f9952f484272bdaeedbe1a0000000000000000000000000000000000000000000000000000000000000003c80a0fb32f488aad7fc05e35db190118ff827b8a233dc02cbb601c39880ef37d58c45a0339b4eda4991d0b6fdfbc6d14647c4aa65b2be2338f6e7a8cd00891011ff7f67","0xf8e80c8456bbcb5c830186a094b02a2eda1b317fbd16760128836b0ac59b560e9d80b8806332600060d85560c56046536000604753601e604853601160495360f0604a5360aa604b5360fd604c5360ab604d536048604e5360a8604f53602b60505360946051536000605f5552600060c7556142600060335580600060f7551f1994fb0e6014609453602c6095536023609653607660975360c7609853607760995360418360306ca0a23244196d76813dba15fc4ddbab515a84a48d7ba0e7958d6af8425728b90ccea045b3062051ab8ef8e869b848aba24283c565352c10d755bd627588e5e6472baf","0x01f8ea83301824088456bbcb5c830186a094b42a178330617f4beddaf629af94c18a506a84b980b8806000603f552d7f7c825a475d54e7d461aba177acd7aa495bfa1476a575d5f2e52a696cada1b4c660fa527f9e9795eaefd779d15a0c58b96dc61d4b23b0196a5f80c962822f9a655973dfa461011a527fcdcc8a0350c79df5dcd4a38cf7ab9cb925674b8e4b9e0a4ab8139656a7f251b861013a527ff675162f5168fda49c9ebfc080a0de810d476363e4cdb0bbf0003e86da93a49d75b4ba27ffda0cd1fddacbd92008a0745786ab2b68e08774b5c8d86e98a94bf8ad41cf79a1cbe740de219c1a3cb1ab","0x01f9010f833018240a8456bbcb5c830186a08080b88060006046557f7c6036a93ed43213f775b85bf125910d258a32bba7317be5d84fca7e6be00121602f527fa533c9ce0a0568327f8f4b054960b3eecdd281ed88217cca69c644a99afb2f94604f527feffe29404cec1f706d896c2d69404e466d129c6b6b90fde20a86f0181799ad95606f52605d608f5360476090536000604655f838f794d7b53f7c9113eeb4d2efa08826a8ad9fa68d9c50e1a0000000000000000000000000000000000000000000000000000000000000004601a0f9e7dc1ff4ea31dabe93304c733a6a17f38cd4178a36af4d48dd76397cdf7697a04c1fe14aae80a3f306d15dee381ca58d99a3c55c47fddb13103b1ba2e2637da1","0x01f8d6833018240b8456bbcb5c830186a08080b8807f72beae20b05539febcb5adb579a3f84b9e0d55a54210a3a287d8e61079001432605052600d607053600d607153601360725360cd607353604e60745360b560755360006059556000609255764c4c603f60965360016097536027609853603c6099536093609a5360a2609b5360be609c5360fd609d53606e609e536058609fc001a0abd7aea7546ac5f81977235084e0801ca97cf49cddba2456ddda8f20759a92daa0456a092836b5888cbb2557ebbaf465cb2d29d194db9a35a224eaef7eb83ce176","0x01f8ea833018240c8456bbcb5c830186a0940d9bb71c273b59764bacea22365f600c8546f3ca80b880927fc4df8d0166836e6e7082161875dd2db922285b788e5d752aa77cbe3edddccc8f609d527f694bd527e939bcc8661507e116d6961be4f1f9b4619a2d7579f9e351e9c28e3160bd527f2351cc13b4f1e121dd19083e9406f5e0b286cefc21e17efa4971726d97353e3360dd527f110170c9a02e7a27b392470b0c0b2e26f6f6c080a00141f3402cc327334f7e80af3741d6d290216fb1bab90d5a752eac88cbae43f8a06efd98e29c973419de8d43357bdd90ca3d53d2ac75a34c12741d9cac8c5db115","0x01f8ea833018240c8456bbcb5c830186a094992cab9d73b86c3edfa62fd89b185fbdd433ca8b80b8807f1ae37970feced018802dfa3a9cf15566c42bbac4129409b4a9dc87545033d1556053527f25ad87ac804f029c7bac96cf73cac3c9e9edbf7b3c36789bd941efd19e2857596073527f46748318359b1f0144090ddf82da916ad6b2becb037e84d1d17a37cf4e530f8b6093527f8133b8180ae8f7ac69c5a0aee273c8a5e9bfa4c080a08a8109909c364cd1abd67d81a309a075e3d5d5e8640bdf7b6511fca90848748fa01d52a8e21eab3231caaee4cafdb0cd5d04187443832113b27139810addd337f2","0x01f8d683301824098456bbcb5c830186a08080b88099d5becbc19612800fe0dab560d460875360cc608853609560895360a9608a536087608b5360ed608c5360a7608d536031608e536077608f5360a760905360fa60915360b4609253606a60935360db609453605f609553607b609653603a60975360db60985360206099536046609a5360fd609b53d1600060b755b460006003c001a0ddd75f794693b6443a6ed3a3f8fc6a0601adcf2743315a43604fbaa5a88d1488a023ed24f0ec4fb65f9fb8a995809efb569d181ef9a06082b41a3d657709fd1000","0xf8e80a8456bbcb5c830186a094000000000000000000000000000000000000000080b8808d600060d155600060f65560006070556000600555ae7917ed7bb1be29986a6000605455506000602a55600060d3558860006035557fea1c220de9a571013d71025419f3a7f497bb6a08ba3314ddddf62a3b890265516086527f2576e6f79ea65c9a68985790d6bb1c1bc51f74f0d00fcf9d22cf57d4dbb40f8360a6527f23a98360306ba0dec4aa2ce2065e5ad0d707ec3c4e2d863d2310a40b35da95ffe9a75c9b397284a06cb5e96ccdbf6b80f9a68202a032a86e06f5e9a0580bb81a79076678fb93cae5","0x01f8d6833018240b8456bbcb5c830186a08080b880d21d156000604d557f86e1dfc5306bacd150665884e35b6f69c97f6e8fbd3fceee417fa4aefa9769d46066527f227bf73f872f92cc7f88fa9d0a422ef3f25d13cb846b080f605544b4f52efb416086527ff4f8d6d453ba8c2470e21d75b26b43e381f449fb43bcf58fd3af8edfcbc65c0060a65260c060c653601060c7536046c001a0875ae21096fcc59e9019c593931d370ebe84328e80ddd4cd2c07d0249d740530a038db531e8900330cb93863cd2935db44d39947672c54130d0af96d28534cf585","0x01f8ea833018240b8456bbcb5c830186a094000000000000000000000000000000000000000080b8807ff1fefa61a894c26f726837ff521a2cdf05a6476d67c48f77eb2580833d0e94b96041527fb8f34f03e39b91b5e259d3bdbd70c89d4666f9d835456370cefe4996409fdec86061527f44c49b1dd3e318c3b10d840466d9b177972c60de75ee27558a20271916abbd926081527f14e5e9af39f9d02ff387f8e9769182fc14c209c080a0efe1a23af9e6bb6b8dd48c5276d5101cedc871d66b6548e226eefe7d13faed78a030468c139203090064e4a46cdab480295d3457862570f88f631f11ba7c3a8ac0","0xf8d40a8456bbcb5c830186a08080b8807f75a927dcb029e4d24d09aed10f22bc9e03965eb8f50d2312f97d99a08a688d3463845d6c1b527fd77cce7a63c0cdd9ff134fbddb897b1da12f00b3269613af2cc809f198a8340063845d6c3b527fc84b85f6abe8177ebf8f96950eebd1fe29e24cf00a9433b90a623cf26cd1abe863845d6c5b52609863845d6c7b536001638360306ba0e5a5b41c7c756cf4b1ddb2b639ef008b26a7f111d85be85088bab5bfaddbd85da067a240fabeef10a453124961663239d8dd32e445ad7363a4432022d0d94141c7","0x01f8ea83301824088456bbcb5c830186a0941af9495adc31dde941747dd0856225c4d8ac591f80b8807f066dff32b261f43f85aa7aa9cb0df3aa5ff5985e17379a6aafa07e17b80b18fe6000527f8cc467c3537f10ded1596e65d5c25006d674536c9495799b5c2813d03213af996020527f87cf5742f07ca714510570eac80c0f629794f2c0d44e0104a97196943401c82f6040527f61f508e5db094b65639688115c2f826aea8535c080a0ea86cb000d52fb35356fbc35ca36cd373a8fd9651ef813e86cc649305161bbdca00e0d0e36a8ffb5213c9282fbcefc4ea3bc874714e21ac6351e526a6c17213b30","0x01f8d683301824088456bbcb5c830186a08080b8807f7894b0024476b42718cf62c67eab93bb63e22a0ff9c62530f0687c9f01f9c7ef60af527f0d2d3a8f7995a13103ded604c02e6eb9f5f97da8dd66244acd34ab7e9ee597b360cf527f5697f0ad4beb0606d07eae9359eaa7114a677cd7c387188dae05c98cce287f4660ef527f9385656bf7b353fd543dc89c2459acbddb8b05c001a09366253665ed0d8fc75f91460432626d912242e7a0e375301b5179044d2de8bfa04783095f646b0dd28a3f3d15529ef8097c6aaca401a2a24062eaca69ebe808bd","0x01f8ea83301824088456bbcb5c830186a094e52ceae55b1cf03740b365566595b641e763494480b8806e77426b7f09bb183f3a17287fa4a5de53ada9e630f710560e1e9219565350006663193f7160e7527fe97b0d5ef175c83c3828a7ee7f0e2938511d380c8779fe8bbee1737ffa8104db61010752602e6101275360c061012853603b6101295360a561012a53606961012b5360f261012c5360e161012d5360bc61012e53602061c001a0201ffacd3c0ae341a20840e8f2ea79d001390b1156e31e096e150aa0bc1e0872a051c70608413e727e1ed918b76d0040fb73ed6c2842edb221da87384fc6c63311","0x01f8d6833018240a8456bbcb5c830186a08080b8807ffbfab03ab105ef9a97def25614375778a9cfd1ddc1690c9f48514eed98bcbbe3605e527f0b3a8c61117a4c19a9bf6a460fb3834575ae05c1e04ff60ae1b93f0696831d53607e527f92e96908908fe88834081ba23ea085639c840912b1253925e9c776fa9d39c0be609e527fd0d199be5c5503f5dcc8dd07ffe2a0a205b98cc001a05b455b55467b151a7b08b99b527ada735b26ca9913bd8d868d40dee428bd1fcda00a5325b0ccb63c210611635914ab5eea06ab893d1e176eab44cb0528a4584e1b","0xf8d40c8456bbcb5c830186a08080b8800721600060745501a5b80e7f23a67bd566485fe65e5b98b360d2df7c19c52837952a35b6ae2d22d99e50b31e608f527f13b74568bc514607f1058bb6a1402ed4a88a5f4cd043f85c1dd6e3d837b54cfb60af527f448414e37dffa5939471dcde75121abc61e0b02550a383a6ba9d626b2538849560cf527fed27eeb49374d34d8360306ca0bd391f0c77fd32cffa7dee8f28430bf9d77072e722a9454083d6f7a724ab7b54a0451b5424b1bde3dcecb798f86f185e8c75f26a790ab8477ad74730d2c2a41aa6","0xf8d40a8456bbcb5c830186a08080b8807f625781a9b53f54a75877a390b65cabf2fe96f4e4ce436c156347d32cf5558983605d527fd53fb6e31643ff38bfb37abaee3dad10637b991365a56e297a1156456ed044e7607d527ff18efc324e94934eceef127dd0d8028c73e05d94c9d3aff8755aeac2e7822d10609d527ff90edfab865d7d18da5d566f5249c37a2a26be8360306ba0dab454cbaca92b13191a7ab2936f97a0c27633be7de142aeb8bf956327f42017a03c68a2dbed6e9e78e35779636b50e606bde35d79344b8d25f6263de375b825b1","0x01f8ea83301824098456bbcb5c830186a094af19415ea64d3c7236c0820a49609940ccced10c80b880600060d155b004e6aa7f730cc31bfc8e7cbc6c56bdcc030d46dc22cfef6ada2f74931d1e38083bd9bc16607e527f57a2428ce95015b4c6c3902dd1d1903174133feac3c2671b128534a81a998150609e527f36bead0a59563f0078d73a728c811ecad6c364a43d1c29ee5b4535497318b20860be527fcbfd50127590a2bf1a29c001a0228078b32ab3325be2dc3b7ed8b4a654d9944fe93b552dc4eebd492574c70097a00e3feb04c72b865219453d69ce99401438c5ded5f400ed6b266b22671b6c8644","0x01f8ea833018240c8456bbcb5c830186a094b02a2eda1b317fbd16760128836b0ac59b560e9d80b8807f0c74fa9a3dc5b36cd35dfb18869e157632188510ba1b7f32ba837a4182d6b7cf6024527ff7b100fdc4cb9899c75ae55a41084aebb90984298a3cce3a41a01e341011f0de6044527f0ff89e2924f4313ddf55bf4d340fabf1a8350468704c900b10fbcde9e6ea78de606452600360845360f260855360816086536074608753c080a0e845cfdece4eb42a00b4acec059f4b3ffed3c700654f622676582e7ae79ee323a0215f856c625614abd7db6d7a485904914e0a011ab17090db18fcc8f71d8c933b","0x01f8ea833018240c8456bbcb5c830186a094b02a2eda1b317fbd16760128836b0ac59b560e9d80b8803c600060a055716000605155600060e855602160bf5360e460c053602360c15360c760c253607360c35360fe60c453601860c55360c060c65360fe60c753605860c85360a160c953606360ca53606b60cb5360a560cc5360e760cd53607460ce53608960cf5360f660d05360d460d153602b60d25360f060d353609860d45360c001a0c47ce19b0c1d328fffa17c52c303f33c1a88503951d9b65888ca9d3153a15644a045e46bea73723acedbad702ee64b9e13135351a5a919360eb1e053b02a5f8f42","0x01f8ea833018240b8456bbcb5c830186a094b02a2eda1b317fbd16760128836b0ac59b560e9d80b88072600060d0550696db7f3706042ddadc3e32edbe3664d352a9694b4ee528f72cbfe8d87190d90534310b6067527fe04f58cbdda256fa02f1e0702dc1ed05f92af0672ba9370dc16b18d1146ddab96087527f6703a1c3fbb058230312b242732b54249b65101338d465967a22a3927286ca3b60a75260fd60c753607a60c85360c080a0a6968be7f54ca7bc8fd0f135bb9d03757a6957b3827e15e3757f05916f65a6e2a05bababc09dc3accb3c7a17672c27b98783a2c7c4b3cc7271d1be2852810a6237","0x01f8d683301824098456bbcb5c830186a08080b8807f5fec3ef28f7a6ddc1218ec4e2d142f23016e9687e992e20b76c08660e99d9e056073527f4d553bcb1614b6236a880452578147e138ab08aa5f6efd56ae608e3c681abaab6093527f8e9a2eb7ea8fde3250e86c091d4f06b7a36003b146bd73b5068f2d0338a0911860b3527fe4dfa0d08f852b1e41e1c188c128c2eeb216b7c001a0857f91cc6d4117e9aa3eb8b5a38cc48896a298dec4744b7068be42726143ba34a06b68aaf1ef36aec6c673cb128ee22f87d3e98d0968cd3cdd5d8cc779e8c8b7be","0x01f8ea833018240a8456bbcb5c830186a094c1e18761290aa08e47a16f0286f8c02be494193f80b880600063c1702db555600060835514d77f05b988e30588c79af8c01670ea148f0c8e7a3c8ce05fd0f105f550c4006871a560ec527f61b99a700e7d9142f219dfaa501307fa7d802d6564afa55fccc5b9773fc130ca61010c527f4a641930f594d297f550a62516e08e3594826e825f3153b0ff0582ca662ee28b61012c52605361c001a00745b845bdcc36359b29a15b2f04fb53bb4f65dc614e4ba8ee228fc763a3c999a07476db91b03320271909a8d6a05c96eadac3a3585b3aba0e22b87d01e2b9b6a9","0x01f8d6833018240a8456bbcb5c830186a08080b88096607e60a053605360a15360c360a253609b60a35360df60a453600e60a553601160a653604f60a75360d360a85360c160a953600860aa53607160ab53605660ac5360d760ad53601260ae53603560af53600760b05360c060b153606d60b25360a460b353603760b453605660b55360b260b653b96000606955600060cd55dac001a0f1c86dba73596eb738c1f0445854595bb4a978075b7cfa09966ac3e1ad44d55ba03b45f396c91014d4780c00ed105849e76d603c80ee74ab13b4564fe57581bee1","0xf8e80a8456bbcb5c830186a094f200e76f66b0485ad55ad8c7ceb3b0917c70908b80b880ccd27f1da9f93d4e23bffce2f8d6dea2d215dd20bb9c44eb8a816a215db51369a16872607d527fe712d6c60f91083d3c02883d5a15fcf8484b31b07e53f6c2342dbdf9f9601c05609d527fc7ec69daabc03a927c63f79f2e180a6949e1be95672b8c7e8cdd801824dc9d2a60bd527f5e67b3cda54c4fd417d5f08ef6bff495ce8360306ba0137706d46fe5b929b1442c32e2eb6cc63f51c045ab47be7f4dbaf53829fd4acba015ce9ee5c8903bca981544e48cf9ceb2d3d29d4d78639eae936d465c235b4c5a","0x01f8ea833018240a8456bbcb5c830186a0945b5e48fad3933081f9ee1a8db13af9bc5608cbfb80b880507fdbcd50d27176b90d4f85390a3dad696aaff622952ba8178c760ecc1a60e61f46606452603d60845360b56085536044608653601d608753607760885360b06089536086608a5360cf608b5360d6608c53601d608d536076608e53600060df55600060ae553d6000593e5960002080556000600955600060e05560ad608653c001a05c3a0a94369c231497b498b0d96a157363e6767e76e9374e4943826e85f3062fa05c3e953da03d33b272659de6cce6fb872e1a85029016f0033bb4127eccb0aab9","0xf8d40d8456bbcb5c830186a08080b8807f71e3191893ea045a00322b812a1b83875bfe5254796ed52fa1ef168063508ff56075527f24f2a10d2de1db5d8921a9ef54b7f3ab135eef3f9bd57e4905cbe98f79b1ef396095527f199788d90d43f498af89b5c149bb46d546b0ae0ec12588a30500e4f9e3e85a2360b5527f7105f16a67cde10619e746bf3a522eb30001e18360306ca07308a0abccd0162dc37d4afcd901ad1a9bcc46eccdbec39f0e448aebebdb0d74a016093b5628b53f7b957d7f1f4cec1f7031012932e5c519c054107b3c844e3aab","0x01f8ea833018240c8456bbcb5c830186a09427f8688710cf39d3d22251657bb05033b625ff2580b88060006096556000607a556000600b55559f6000603055600060a255600060fa5534600060bd556000603f55047f3a9aa59dd7c788e537afe587bfaf86b818dad2eb8f27738de94f90e71104037b6006527f787d67618521142828562b27cf617303ee8a5272bd15a1c458d8c6323520b72d60265260dd604653603c60475360f3c001a063b72142aa9e8a6654d799709a9e05333945e5d541fe7f0cfbe5e6660ba75ad2a066efb092ff1d169dc8b3363ffa1a6c11bcffd6aae95b799d726774707073cee5","0xf8e80b8456bbcb5c830186a094000000000000000000000000000000000000000080b8807fc7e8a883c375a5bdd931e50e0d21118fe674800fbb6c8966a88faf7d2255f7076029527fcdadc43b8dd009a0828a3afa599429354d1a05268aa393133bb61a7929496ef46049527f806fe4c0a58433eaa12ee76f516b454288902fe2994f89abaf820bb19314a3866069527fa8d8bcc67ffeeb4b524ec6db6b3b935b7a91768360306ca01dd4caa7ba6f71d8d661c1394e461ccb60121d00b066cb46e9a1e752678f3d12a008e3b445e3a18bfe304fb84438590728f8ded54492250f0cd847284d676527c7","0x01f8ea833018240c8456bbcb5c830186a09400aebafb595c4ec7889f20e57ad9c6d0d427711d80b880a860006064556000601255346000601b557fc34a1ac482b3ff08708fe52de341321eecf8c8400b820c23069d4c977daf4a3a601e527f93c1d75bc20e1486dc7876019a42d8dfe762c423f8970c86866bfcadf1e97908603e527fcdd3c81f26c93226c94704cd43988e2a58c357a266cd94ea01416482ee770d7b605e5260f460c080a0f3a1d1908b1cc673111cbee3b34f0cc19b6d685dcb2c3c4990ae31de74bfe077a02f36b414acf88e5f07433572c7562721ab9b27f5a97ca574a7d4c99c2084247f","0x01f8d6833018240b8456bbcb5c830186a08080b8807faa4d6eec613ab6a3c4b8d2f220da60bd6fe5e12ef766ae4e3787b83f62ae611e60bf527f3b552643af16fedf02b80a1ab4eda3b6ce1b7b90715f5137d0c29fec9db88f0e60df5260d360ff5360866101005360476101015360e26101025360e66101035360656101045360156101055360096101065360c461010753605261c080a054bd4ccd6478c21414f7bf469489590bcdb67feafb36cfca3cee7e6ed402035da03a8708e6221f78019794bb4f4868ba8c82d2d0f91c7059597d2c186306b09bd9","0xf8e8098456bbcb5c830186a094000000000000000000000000000000000000000080b880be60006096556000600f55cd7f9c7a524897ab657bdfbbfbcb695d90653fe1b18d2b2bbbc4d4e89402497c9b5f6081527f8908669b246752026ce758a8c8010f4d83ac85bf6afbff6999fb31a02ba0dd4d60a1527fcd98ea767e537b19a32a5eddd6e56a7158dd92c3e6501f1be4525986f3bd323d60c1527f8f48e256b822c28360306ba08e4b9cae54c1c3bd574bf631128c03d36ce819fe0c0e888acc86113b0e43c159a075bab053879275475e3124e5ff17dd4d1a64c30243651f473c2521cda19c620b","0xf8d40d8456bbcb5c830186a08080b88060006088557f3d7b4c65d9513b25a1fe6d4e06db752a2ec9c231520fd32d06a18449f2bd1b3760fc527f170ad20424efa4e478da4718552af3b1143093bc780f6a4897fa7d29fafb18cc61011c527fca58757f032a2170dbe788f41e5ec152c91f584bb3cff2f776a80deb3345066761013c527f712accc7db1d82ecafba2b738360306ba0fd23fbb79be7af23a5bf991925862689320be26936d9ec5688ea079a7de848e2a0799e182be42631899fea71f18f591060573f6f18303adeebc3f278e72fc5583b","0xf8e8098456bbcb5c830186a09422e36c3909bdb340f6b64d04274c81d4cc4be6b580b880c8e4833f600060435560006000556000604b5504600060f55560006079551f6000603255ea7f062c9cd77c16d9963307b275fad16c51e303127b3ec1e670e1dbe9104768b6086010527fd3e84404b56e4ca73b7eb27559ef26b1dce171b11350e3323d5fb90f38758e386030527f69c565ad6a2d17a24895723352c1a3d44c698360306ca0706090e845fb5ffc92720e608c111e1bc8e2afa0363a79a51361b9d037c663aba003eb03a1f1603ecedd0482ee59009d36d1c37a61a042ceec62cd5aa008ba7630","0xf869088456bbcb5c830186a094000000000000000000000000000000000000000080823d0b8360306ba06c2bc19019ac35e3f13e2bed3c6f13864f86b24500a67072250d84bccd2d0ed2a0631cfeb0396e1b30bc8367aff70b8bc78a3373d6be07dd493f9369b1c0e66994","0x01f8ea83301824098456bbcb5c830186a094ffe4bc6bb838a0e9678a701c183b2c69059f0c5580b8807fc353b39ae5d72064091002ed81e18fad21bde72551bc67f238c3d48f23c069f260fc527fc9e048dd5d8c04f6d49c81e238601f41a327beb50e9981c1af9ef8f5c54e5f8b61011c52609d61013c53606361013d53608561013e5360f261013f53603c61014053604761014153609c6101425360446101435360c86101445360c001a0284e527aa0ea56ca791e1b8b5c3edba7643c0eb6d60121f20d9cce1125d88772a00ab6708c6a21a64fc758451ae523c599d6f0b8890beda2b60ef7dc0a5ef49085","0x01f8ea83301824098456bbcb5c830186a094769a86b611010e4f989daedac3d15b1f2d32b89780b88071d8757e03b72a112234309281f0678e63005e12a5f12feafa3a68ab7eef5b1b71465e6052527f94c46d7133958981f9d74127379117bbcb60d97d80fb72e15096c640fd417af96072527f02bc66a35e3d7770a77e96606026a382e9edd1666e9808488a14cd64b8b0697f60925260fc60b253609560b35360f160b453600a60c001a0b3b4620d9f7efa89f8f9b69aad0a2bc6b20e4bfe2472a34977e7613036a9738ea07e2d5b729d1c1401b3557c5199e89ef4ede624819c22383c882eb28c9ee2ad5d","0x01f8ea833018240a8456bbcb5c830186a0945bc1de52551f6c5cce7926eaa5453aa5e27b639a80b880600060da557f7b1750add3f29a90caad5195a31c9290ac11b934dc81d0f79d17aff2a4b61eaa6024527f83e653e89123e298b671d433156c57c9c01696850f0da7a4c6ec47a62ce258b16044527f79a4261df64744ccbae802009360cf2e1a23d259c22eab2481d6201d8f30690b6064527fb2c41c0f37970bf99d9c95a6a380c001a04eaf50d107e6506cef86b775298c4dde13911c009986f3c85802cb827324039ba0489429a83b9765028d0f9d59b765cbeca789dc70abb9f17f064ec8a2203f9a45","0xf8e80c8456bbcb5c830186a094e9b7c3e579436ca522a191e3d93d625a9810e5e380b8807bb84b9f6000606155600060c5556000604d55c57f3cb6b2fb71a936ad49493d74a475d6ed5187decafb9e54ba61352b0cce87a8a460065260c160265360dc602753601e60285360eb602953604c602a53605e602b536031602c536090602d5360f6602e536042602f5360d460305360db60315360fd60325360f2603353601c8360306ba01ec5c80b67ec603bcf6a39a2b2b79050d61ef10d0f4c1f3c88459a3c6da1c128a0365d9172da669678bd36f7b86fb63076e1d190c439911704c61c9fed0e7e3cbc","0x01f8ea833018240b8456bbcb5c830186a0942cf5edd3d884cfc30283a937490e58dd48aa061180b880f1603060ca5360a760cb53600660cc5360ee60cd53609360ce53603060cf5360c360d053605c60d153609560d253600460d35360d560d453600460d553603260d653600060435560006031551bd4600060d655600060e455108b7fb353ffb4a6e9bfe2e02247210c493ab1404661740b6aedc0feed1771a2f25b3860af527f70c001a09856833e3934fa30ce27b92eedfd089c8f2042563fe45b08130b1ff896c34e1ea0459464db37f227f4327d4cc71a4bd54e892d0cd32f7c44e26174fb31676c74d9","0xf8d40a8456bbcb5c830186a08080b8807f52e9a4600c39979f2de61d147a2757679cb986bad0a43abe36ecf9e7706647fc605a527ff749da08b33228da6d4d7122c56b7db45551d43ece1461cd827f46fb761be360607a527fb68323eecc90ef0c7e0fddbd9f3e17a24d3917db87c93bd1649569490700c95a609a527f403385aa82f910ae9a14d576c516f68d49871a8360306ca074f7a0b19ae77fbe4fce7630fca856121c3d460cddf875f944a2d36a8bb755cca01bf328a41f02ee93efbc450bf2583863e27749a5368c69790469f818055caaff","0x01f8d6833018240b8456bbcb5c830186a08080b880f960006013557f800c16682b75e3a80af2e68c09b314354683c859d554bf61d20b97780a2f9922607c527fd9eebf7c125cd572c6aa7f3d4351edb3abb1dfb8740855db3d644132b58d9aed609c527f9f43637d92414d299027cc4b2c9c7bda9a3f010348f686dfc9d8efffa6e4f82860bc527fed8536aaa038282d91fc966ea2c080a0c218119e24a86d43e762f2310b89ba236948cbfb82205f2234e86d472da330d3a030a6d86a2124021ebb571bdeab2a92bcd5a6bd2ddad38e0be139832e772f5779","0x01f8ea833018240d8456bbcb5c830186a094b02a2eda1b317fbd16760128836b0ac59b560e9d80b8800c6000605955600060d855600060de55c14a6000600e55600060ff5569bb2a7fa597dfcd7a3b24d0ec7a3f36a57ca56962d9bafe7931454a2f1c09e979143c25601f527f5d76f01669bc423b2925318c2416804eeca63c89501f7afd8940266df1340cf2603f527f23698577a4ed91d65a29aa4fd011e1d6cea4076791c8a522c080a0fe2f432c55d681efd3a45bba8af93e440fcb46132abc1377717e2d00c12e11e9a06ee5f46bf62e72194afe4e57d6928c0b28fda75769a30cf8b78192b2303b097e","0xf8d40c8456bbcb5c830186a08080b88074c3a060f6631c4a583c536027631c4a583d536067631c4a583e536056631c4a583f5360cb631c4a5840536035631c4a58415360e6631c4a584253605a631c4a58435360ba631c4a5844536025631c4a5845536058631c4a5846536088631c4a5847536024631c4a584853609f631c4a58495360d0631c4a584a5360a0631c4a8360306ba07680a55782b15906b9100f44793b5ec78943de8aea58087cd21c18fcfd153aa7a0692eca4b2105f4bbbdf7a813ebcb768acd5d29ce73a832ab296dd203d5623fe4","0xf8d4088456bbcb5c830186a08080b8806af4566000606555f67fa119b073e642a6e1ced795b17d0be8185a0f70abe7c0f31ece38d32cbad9a76a60e0527f163be02009dc6c25516177df0405d44a3ca182d422cd48a269e706007e56c0b7610100527f7ba713de2d56bad553be2423a166b1fe90c902b60bff55c0d0d3364bbb56521d610120527f576fe7aa1e106b218360306ca0508a5f22f9c4a45b0563f88a349e9301881c1b9daa3ab0f3854c1005517e037ba05c425db9468cf40f56279b6c8d9d05aba80dc5fd751f875aed1b64458fe0ed68","0xf8e80d8456bbcb5c830186a094416fb152c3bbd9458733edc13fd0fa6b2ead280b80b8807f24c237b5a0f573e4d3781b28a8a8167eaddf36552ee20bc80fa44909b3c8b6ed6017527f043ad95570cca3f8faa594b789b52e380e9004fbcfabdff5bf7f883289324b3b60375260ae605753603860585360a5605953606c605a5360e2605b5360da605c536058605d536071605e536082605f53601b6060536047606153608360306ba0992cab4675928d1482ebabb3beea0bfb7467dc091ad56fda22ecf7cb639e616ca00ccc1496c6ffeb9f4abf7bf9da16f8f786a70c73e3a859a791e2ea5605c41541","0x01f8ea83301824088456bbcb5c830186a094b02a2eda1b317fbd16760128836b0ac59b560e9d80b8806000609455927f860aac2a522ad0d01c8fc40ff419804e70251b85e1a20445e06ee3f06fc808226036527f4a74b82ab787b646a99ba74bdb58233886a571b98e758199718e0793eff8617a60565260ab6076536033607753600060ea55600060f45539357f0b668a0d1189879e4ba1dc814319b22867caf521eb54e392f04beec001a0e33db291fd057449f272e615c2b099a1b6cebc745dea269c2e633e9281604bd8a003ebe587b59ab7dfcc0e7550b8daa1c87416eaaa93349cb41d8e2af0e71d6ee0","0x01f8ea83301824098456bbcb5c830186a094000000000000000000000000000000000000000080b8807fabca9bdf94c10b871e42dc9c294bd68ecc0033484291a5157dbd85c0c939658c60fc527f8b114f3170dcd8d2de2c5307194643f3856100a6425b5140dbd7e6955773a55761011c527f1c5c15b5a9dce23aaabdc2c6b407836aa9ef0609a8466462589aee792ff6694761013c52600861015c53607761015d53604561015e53c080a0041eba77c3f5c87110032414aaee34cfde01b3b395d239876e392dc9c264808fa00b0b91ebf83f40fbcccc49b05703ce22e85735cbeb3e3c6a3fc6c0256922cdab","0x01f8ea833018240b8456bbcb5c830186a09407cc8591470379a4079c78f3246d3b778a764d7280b880db13bd7fca22afe030a63c9ce8e99f082ce2a451d92c63a719601bd7bc810803dc52338460f6527fe4a08b6d0f77d3beb5102da60a1c41ad38945ba37204739038dd32a50dc514f7610116527f67a26bde340c1f0b378c805a11e5c3b76c4cc1ea89eaa53d414a969b21d03aee610136527f03ca63834ab60d307f1a42b8f4f9c080a0096dbb4a16c89767522fc4fd6650ef4a53d068cb2149c5020de049d47c6408a6a064f5845774690df8b807c930500ec5d00eddf0adb25edde1a6cacf43b5bd18bb","0x01f8ea833018240b8456bbcb5c830186a09405add44efbd3138ddb2a379bdc949972d0e0fb8d80b880600060c6556000604855600060675560006094553d6000593e596000208055600060d655db6000600a5553016000608c559f38aa600060bd556660006020557fe8f83c6a22a10569c757ab95aa7e380dd83ecb2f665791fc0dcdb48e0394decb603c527f76ee92f3918088eab70c88d90190d334e6ff617b19cc364aefd77521c080a0eb12ec1f93bc26c393fe8db5fbf1d265e81bf866c30ebc0df02ebf9f5721836da036fc498c31ec02331293a175b10521a320712d3af276b989430b4e46fabf876f","0x01f8ea833018240c8456bbcb5c830186a09439ed8dbaee723aeaa300c4c2957d3a2a4c57416780b8807fcdf94345405b75ff3304eabe57dba4d618f1375c0003c58ff64687a8c9171f5560c7527f59bff494cd4f527a3211ab17a38d12d47d7e2cea7b2a971e3a752e20f5f9efbb60e7527f3b2059ec47da9b7652d0a5cd7d665198deeee4e980cfa7279759e0f7a958caed610107527ff2e5ece07dcadbc06da58265bf329ab228a3c080a0bc066d13534593393833cf70d481a41a8272ccd80c59b0a2d9c25f3144d2f580a0575d33110fd1937423d2a708c60b57547f3cbfda4b6e5ca085427b7f0fc0e481","0xf8e80d8456bbcb5c830186a09409c9dbc20182ed13443ce36989215dd874a25f9780b8804860006013556000602a556000608d557fb896b1732304cd32c295fa235812cafb37f93108569ff682158f11a3ca63c79c60b5527fb72d3cc59e354f4463d8f237a0eaaa7bfe6ccdb179a76f2862b790af63d1ccef60d5527fa32358716e047a460122294b10cf7f9c1d004c12da8c71aca8eea86ff275613760f5527f1f9c4c8360306ba0127d31a331bfed381d572c3e1c580e6827874eaaac14719046a7f6e82a7fcb9fa005401b13b2c975e5a49f05b48fc487e59099431376c8982ac7ac9a166d7efe4c","0x01f8ea833018240a8456bbcb5c830186a094e2f78008d47a12393f82de950781b462f016349380b880600060cf55b96000605655600060ac55da7f109d7cd1a9c5f21dd5f818aef027a78aaf115587af2b567dc04a183d55597b566072527f997b0744ce3389ca4bd05ff872f3fe8b5ac315d116e0a6ccf81466e9e5d885c96092527fe0f045e38eeb99347428d52e30f9efa809395d86fc375dd1e9d3535a1a08f9d160b2527ff154c001a01e1423df84df242699965c06b3878172caa43bf0f6a4f06f8ca933bb105d0956a0296c321a9e7e844ff6ade9d56735d4260209be1c68d598ea45703fb2fcf7ff78","0xf8d40c8456bbcb5c830186a08080b88075600060865574147f7115eae95058b3e96b0bee0b97c93024a10e61142c3a2917f56c683ddf81d4ef60f95260816101195360e461011a5360bc61011b53607461011c5360d161011d5360d661011e53608761011f5360076101205360cb6101215360696101225360996101235360b06101245360ae610125536081610126538360306ba0b5a6ba8b50cac976b3e1228234beca7ace0ca3893ff6bf92afb7be5380812cdea0683aeed5f8d062bb72cebef17a5ccc62f9370b69641672bdc9e8da883f1ba51a","0x01f8d6833018240a8456bbcb5c830186a08080b880457f239f9ab59bb08d1dc1988766551dbcf056675246fb58ed79e4a44b3855d73109604d527fc02991386d5444fabb784bd3953e8e8c6cd9291b7d82c225bced230a4c2516c8606d527f162307e7a7a494eb1e082062c8b74f638465b26226de6f899f946a91ec950b26608d527f5484ff46dfee666680b74726c8812ebd7609c080a09181b5265c5149342b16244d48c268df5d26355832e6336bb47e183d46862d74a024df9ffff37a7f64ca07485ccd404405613e1e9e0d9b18c775207cd69b039996","0x01f8ea833018240b8456bbcb5c830186a09493d33207522d00c177145510ba932d31211dc49280b8807fe3e750d9be3a8ab51e0844de087e5516c9c13996712d7eef1030dffa1f75951c6044527f1b14f2b26d9ff0acd00301201608115fe83d1d18c2394c7460d99e10808832fb606452600060b4556000600c55c3b37f4b0e450582674bed5a93230ab27912f1a72d57832bd68fd38cb2105ce331d97060a3527f1e56a212ec80b9c001a0df69c2466b5b934696a38046f01e4e3c8ef8b04e795efa63962847e0e678d702a00eed2ab21ee98ba0c1be0e9f06c2cb2314511d86267601a0a7d44f5a13da54da","0x01f8ea833018240c8456bbcb5c830186a094b02a2eda1b317fbd16760128836b0ac59b560e9d80b880623a4e5f7f4626a7803e8ada31e8b6c1db86987d47b613526edc91a9c8bb26c22cd1000b3660d3527fbe50cf2c824ef34aa46eb463ff2bd2f5672d8c912f7f2b81c86e9d13df14c62660f3527f844228bca9daefa5af35061b2a1904f0a798ad0ebb9ee644bf723383338b40aa610113527fc41bdaa49c77a74158ee8e1eee3dc001a0164bc5eed554868ef6045ebc4ac2eee751fbe728b287cc37e41d98a5797011bca027f6585d5649515c01d015497c2ab44b251ff41cb060b5e75780de81d2e0c7db","0x01f8ea833018240d8456bbcb5c830186a094bc64f4aa5240bc716457cf59768c8fea78f84a0b80b880600060115560ad60b45360d960b55360c260b653600760b75360ee60b853608060b953600160ba53607b60bb5360dc60bc53600b60bd53605b60be53605660bf5360d960c05360ed60c153607f60c25360ed60c35360a860c45360f860c553600160c653603260c75360a560c85360bc60c953605760ca53608760cb53600060c001a0d350964153a484a426176e458dcd4db27ed6abbd76de2af17533b9cb0f9b627fa02333b1b72c67a73cad47e400f8f4d2469f8ba0af6ae6bdf2c86289beb5163f2b","0x01f8d6833018240b8456bbcb5c830186a08080b8807f23834a73dd91797e2cd8b731ff0376a5c53969d1e54c53ba759faa42e42435406077527fe266b882ecc054b7e32ef215f43a58978e7bd9b245892e31266f4c2e4600d84b6097527f69131c6f8ae2f9d15ecc745b625e31a45ee50b4db187d78a703d12c9877f4e5d60b7527fd32193efd4960042eeaa2f571f1f4ffd32eeddc001a0ff9daae42b30ab17e7e5951f474f87415ee360f692fa67e572980d5b825e791da0587245595fa686ad7d25e4ace2808828a7357ca0fc06332e4ef38a244a6c1df3","0x02f8ef833018240c845586d5428456bbcb5c830186a094d00c16ef40118644b40ecb4fa71bee9310b8b25880b8807f1b4238021da946b337bae207cc425d0945ceedbd4fbe51a965e31c7c0a4eed6760aa527f0a1a07af73cec198f0ee353f30daf974fa3224b4cf2f800bc15d71dc3ef5ca0f60ca527f7721135ae8a47b2a866674ac5fdded797cad2675d94d3d3e87cefee7d8b9f4b160ea527fa22cfda90ae0f89da1e34b87b4fa69b530ce2fc080a0d3bb1bbc17f781998538089712cf0888853914996a0177e34c887cf4a6326bfea0469c6f016675cfa8c7da1d5f54797d2c39da2212e049e4be84910e43418ede6f","0x01f8ea833018240d8456bbcb5c830186a094c039bec1f78f90c61ae563407cf6827522261db080b880d6ac7f9fb2da841c939e6dea7cf035d24119437e7d2e3426277c19be627481fe52d2e66062527f18ead2dd26b4dfd712df1254c069d3a800881c621a70bf0d47e6f99c4643feb86082527f15d15a2f96990e165647770d03718431e8110a9eb09bb5260abfcb16334422ee60a2527fc669e4036356f88c586c76b10e9aa694e4c080a0387a7d8d3c350ea22666e55928fababdb698657498e3acf429034d4e3bb600e6a0671aecd43a3e09f76e7c48aa4622c99250d5b16bd70236c664bbf43f183aef8c","0x02f8db8330182409845586d5428456bbcb5c830186a08080b8807f7f37696f3f179b4738bbcad4567b1c289fb399cf6a5abf39e8d3f4a24c438f0a60fc527f2d5081a3e13e6a0e057cbaa413a0228755802520c6cd0fa0522b500e51ceda4861011c527fd931a4be8c4ca6dff6c5ee9d88e4c064e77aa749cb4ae77741355418d59462eb61013c5260c161015c53602661015d53606961015e53c001a0f6b9238e744b9e498d1f6283018cb4e35601d8f64cd73c950b557b04afc24810a03b6700fc4e9c3a1962bae40e4523578e7346344cbcd7b49677b123837a66ecc3","0xf8d40a8456bbcb5c830186a08080b880637f1d2e66dcc6237bc03adff961e5a6a88700040425c1ade4602ac1add925714c1b6002527f77ff47026d967f5831fcff417638757e5e3f4cfcf525a9aef95818e75848a3556022527fbcc286403fb420211255811371cb7a37648b131e400a47999d81e7b56b63c5c76042527f9d44d707a1617b6b4e5ff186c53aed07662d8360306ca058c88417449caeee2c4d82f79ec1a569d3d04a22fc99203722856e123283c5f5a05c1d4110e9cd6c926b9351f5d3951aed3b27da52051db87b32d76e29965a9b74","0x02f8ef8330182407845586d5428456bbcb5c830186a0949e514e034a1bb6863b64aa2ce869fa9e1cfa037580b880967fd92a52b66abcb3d5f6cdb79b613ea93526bfe685778be4d9ab8d567ef9aa6bfd6094527fd615664857a7831c7fc96155add62d134d825a706862896615db5abcc028967560b4527f4ba7616fbee32f286969f4b5a656ea75d86881fbb4026b861faff8871d508cc060d452603760f45360f460f55360d260f65360f260f7c080a09b9869d2e5b7da4e364a742bf12b35b9d5e3fc36262dc53e5d8c8c68e29ab0dba0765870ff0e1c7d3fa7f5fb58875e22cc6e0e0ea04a36042b69088251b0d91f53","0x02f8ef8330182407845586d5428456bbcb5c830186a094e9feb079a00dccea06d42cbfdbb84e6f7868ed3880b880600060d755600060f55539fc3f7fd30024de8dc03c690505b00b28a9860c8ea35e99e26100bb9f79fda6d72d611860d4527f898e3205b67e79f85eb6caef2a2d43679be43656706c9112fec76eb7ba65e2b860f4527f8daafa9ffb5e7c9290e2c147df61ef0b9cc203fb913c8cba4f49a4fa570810b16101145260a261013453c080a0b49a8d5d4b5319c49b01d1022bf90fe32502661df1e21c01f929cc59504aa1f3a0354f487cf511d59c37e7cbdad188c007e5eb1df90d2c35859b1b6d9a1ea375a2","0x01f8ea83301824088456bbcb5c830186a094000000000000000000000000000000000000000080b8806000609c55600060aa557f6857202b3e185fac435e1adb45ccc3e673ba783fcbd507acf04207a7c8eea8a86053527fa0f3ff6dcf17883c25e0739fa9cf5d682283b755241e350b08cf1f4cea983802607352600b60935360bb60945360f960955360e46096536065609753604c609853606a60995360c0609a5360bc609b5360c080a044bf046ba7469a826b2d203bf64568380e881923e2f386d4c968bb0ad8c47371a00c20ac77cdbbcbc0f84b0243ea4eb74123d039c0cdbe6ec1385fda332f9b875a","0x02f8ef833018240b845586d5428456bbcb5c830186a0944680e3b5fcf50a7b1477bc6971d3db837589af0180b880ed6000605b55600060d75560006041557fe1c78e78101b377e60595de449ab6a6b55fb203ea04f5bfe614784623af2034c602a527f93a9c149c85f979d91abeda835c533848611149c42221d38ab124b762f8b0af1604a527ec832205feb4ead1f39a3e20ca35249e98de0d6cdfedbea6dbb8f3cfdad5659606a527f5068a88fc001a09af724bb4ab44968320ffbc9709e5bc183b1863f14c05dc3159e168efa4f3315a07abdd4a3753cc87da4b82932b35e0bbd09287dcf0ab97f43ead4744df92c4bd0","0x02f8db8330182409845586d5428456bbcb5c830186a08080b8804a7f7510ba716de293aa415a9f52dd122892b74a197c3c432996a85a7b480c19aebc601a527fa83d777aa4adadad420957ea9b2f105080b402e4ac18993e806be92d2f908247603a527f1b21b297bed795bc083212c3b11609fc5ebe56c85bdc3a470a7213581fa5e85a605a527f9bf3dceeadf337da2acfb142661c1567a919c080a0167080b7c12bf01670b7a44ca849312e0f6400372965aaec5a6a066311e11663a0665691cf81286991134d7f6938bd71c10963f7708083340a10779d5cc4ddbd6f","0xf8e80a8456bbcb5c830186a0946d44db2351a11fbaec6cf5c2997ed3c231fd5d8980b880600060ef557bb97f35e041da0363d3bf998bf5d61706e830e858e316df495d43354115bb427ddcc36014527f4728b846171c8d140f55471d5d6a610294b88248eb20b6b87616a4f72dd4cdc9603452602260545360816055536036605653600d605753608d60585360a860595360e3605a536042605b536050605c5360c6605d8360306ca0aa2bbac8db28f8950b466eeca1cce6ef8fdc63ba485c4a314cef4fcd94421caaa028bfa59fe56c306d830a0e2c46ae30aebf65b8595918d658ab47fdbf72f65597","0x02f8db833018240a845586d5428456bbcb5c830186a08080b8808760006024556000608c553b7f9dc7febb947f9ea1bc6cd50f978f53e84f498369ddd7019e4a316b2c1260f3e66007527f7d204e295574e88c93ac36d879dc3850f17d9fb8d5fa34ce981d798afcd5fbb96027527f5a6545ecaeb1ebd98eeaf0e0138226df90e51e5ec0d5500f2f4d2bf47e427e2d6047527f4eb3c213c270f0c001a015d301270cdb788cc9281217288211f5e7ca9fb9b34e1adbb9eec21d58ae3be2a038df714e7970474d4f8bcdd04c0dd5051fab24644799d6fc2e868f1dc502a050","0x01f8ea833018240b8456bbcb5c830186a094b02a2eda1b317fbd16760128836b0ac59b560e9d80b880100260006024557f4123764bd58d0a5c36c1ba1d563a5acd87206dd41798a5bf401e69b5f182f6d260ca527f0be34ca4197a42f51a03d3d04fddf7b9b63cd128cba9dde76bcb00987647b09f60ea527f346d57110dc72dc07d504ddbf5b5a90560794fca011fa77a1b14f83d28de88c461010a527fe4903eddc7fdb2d76951dcc080a03b9b24ce94e5ebc102ab0984469fac9f1e3480524da60ea3ca353f6ef17c2593a01b9a275f97f2de2d55a36377a3ab401e34c36eb2bdfb307c08a9ba84f964095a","0x02f8ef833018240a845586d5428456bbcb5c830186a094b02a2eda1b317fbd16760128836b0ac59b560e9d80b8807f5d52eaf37e3414f31c09d5c53f64527d71d39d7f7ec25eb8723123a42f5e54ea6071526043609153605860925360e260935360686094536041609553606460965360a6609753603060985360f16099536001609a5360cd609b5360f5609c5360a5609d5360f2609e53600f609f5360ce60a05360df60a153607260a25360d9c080a0bcfceceedc4a06677e86548c0d4bdba00ffc03437bfea43f1029908be384f824a03d56f538e9de09566502254158ee01bbbeac1236e66b087c7474c95526f1b2e1","0x02f8ef833018240b845586d5428456bbcb5c830186a09473069e74c322cb5361a3762981ed4b89714b0d1980b880600060cc5560006057550f60600060eb55e6c36000604b55eed76000606155eb7f5dd65e63e78bab6b40b1428456d8719cebb0ec2af5e8a7a6df90b1bea087327560af527f4e435883f439f408d970fb635011804d9bc30af9a220018597a7cd9c7cf468a260cf527fc36c64158072143ce3d732a17afd505f5270795ea22160c080a09bf4c682260baa9e25d3bc0447cd98b311ead591ddde61e653678491b370841da011f252f6cf85464cc6f331412d54b8920263ffbdf5e901b04e77230b945a81c8","0x02f8ef833018240a845586d5428456bbcb5c830186a094b02a2eda1b317fbd16760128836b0ac59b560e9d80b8806000606255eacb745b1574b214bd6000602a557f5698e3355389634cfaaf0efcfd51bdc5583ec749d5e3d9d2e4df3efb0d44db7d608e527f02cc5a60f3be91a1bc0ec694268919a4333e3f03ebc860824b7e07961673863e60ae527f0328113663e8590e55fd5a71b85e1ab4b9b5b6a0cbc8bd245756ac45f3bd6ef360ce527fc001a03fa70e50e16eb737856c8a001456eda5f1de6b83ef98415d02d70876067636b2a02892c163b6fbc95664aae7d1bf4c089b8e5724394a4fc8de70b9d4e29bc6f44e","0xf8d40b8456bbcb5c830186a08080b880600060e055f37f990c7ba4d259d1504f9a4cde9867ca27a5976988509b98d85b1a77dbce4c5bb460eb527f54358fec2529da41e114c062a29063431a65bb74b6dfb2e2b7a5311f78b7b04c61010b527f6747a177057fbf107804d58e8330f724dbbac6ed2f4032cf09280db5a0aa718a61012b527f5a8d4aefc8b9cff7fde63c8360306ca0165cf9268139494dfa7871a778de1c2bff47cd07f63b1769d41b563c01b89dffa070d23923ee0f78518e26d27e1e424e6ddec4077c4ceb84efc5b373de819a9997","0x02f8ef8330182409845586d5428456bbcb5c830186a094b02a2eda1b317fbd16760128836b0ac59b560e9d80b88060006038557fe1f8f250ee40f4261712fd25b6bc2b77196a1424be60ba788c9effdf1ef8ddf460f3527fbfdc711df19272545af912dd3cb3c5b6486057d7da3bcc154e10cb9aaed9bd30610113527f70d892f3e9d3737e5e12b585bcef1c6ca0b286c6aaafa4f88ae4e4c5715a3fb5610133527f9ac1906bbb64963b7f3f60dfc080a096ede69ed18cf0936bac5a8d4d1c898c280c463c7bb2d324c6c828cefb01a85da04d745cc447bfc79a708dc83a279ae360297556aae7b772b098c119b0ca45b7ed","0x02f8ef833018240c845586d5428456bbcb5c830186a094bbe3d0cdeed80f7dec2911fc14c6f19a28dfc0db80b8804b6000603d5599783e6000608e55046000607b559c7f554770f085d50dffb473dea26efdf23fa6966fe740cf812094e68f978ef8fd4e60fa527fc2710f6e59f9f5b722471a3c108523853c6fcf3e9c0e0a205075992b76d7fa9061011a52600661013a53602661013b53606c61013c5360f461013d53601f61013e5360b66101c080a059964610ceebbab7ba77dc2aa8a6e556a5efa39330417f4bb89108ede2615380a01837eb6968c22863b3b8a153c8140f78b65b98542e889a5e7f6cefc679136676","0x01f8ea833018240d8456bbcb5c830186a094e30bdcacf6855ab4657e3d073af1b59365210b9280b880a27f75aa2a6ad04b58f4aebe616c29bc6b5063a2dccfaafc3c263910405b6bdbacd360c0527fd467cd630ab1e892b9e3ff602029be9916b8feb4d48634850e3cc11909bf265760e0527f5e7c1a72b06356c49caad82b40b8744a8af38fbc7739cc9ae4054ce8385abeec610100527f437dd972622bc0d37690b20a14e7b1edbec080a0945a476d34fb2ad411469d8ec866e2841f5a99da4433e34d7d31942e4386b5e0a005d5329586dfa8b08ce1e0bed4f0a8623f1eda11ef55d1b49f08c88972672862","0x02f8ef833018240a845586d5428456bbcb5c830186a094000000000000000000000000000000000000000080b880efff600060b3551ad2d75e3d6000593e596000208055786000602655600060cd556000604c55d6eb3d6000593e5960002080557f0bed8a27866fbb24e38169761272b34e714cd614fc64771de74a7fe7b44cb721607d527f637d2e3a556e40044aaa223a7df1d3360e4134bf82246066b3d807c6f92e594e609d52609760bd53c001a08bd1f0853029ab7579910d41904bc55e954044fb613ba86755b4c702fe549846a038f6371c6658b6b63914202d8d22f028210fce6830e2c964b81e730aa3e9562a","0x02f8ef833018240c845586d5428456bbcb5c830186a094b690237c7cc18102e9fcbb6a9dce47b257ea002480b880687fd7de55d620da0d7d29f6e1a36652a986b039fa402fa18207e0dddab6598a5d4c6056527fccc7019d670512b0e2675513221aa92fb6075defefaa15719d4c938ff3f0562f6076527fc030515f7b788b893f2cf0b399560f234c44e1e24c9ea2ee7b4f33ba7bb059bc6096527f9f17752324538d590930e8cd75866feb3f75c001a01254aa71360c36cd45d3f9d8f4503a75553ca7724d9d784bf30a2067f655ddbba04a654da979f2a27a4d636e6997170e515b71b765be401a51d1eb77bb961a40c8","0x01f9010f833018240d8456bbcb5c830186a08080b88060006054557fc0d477d51cc313cc39bf18d63f28dfc3ad1fdca4c170efbb03d03ff6ab54fe1a6048527fda812cc13ddd4c3b5af4f7f805dfd8a8af5d5434a00122cb447a48e9f9595a976068527ffad7e1b83dbaad1fc376bbaaaff4971ddc9555333f09599d3febb85512fb5e6c60885260dd60a85360ae60a953602460aa53f838f7943756f721a7af9d70786c3b943d1f88fde43cdb5ce1a0000000000000000000000000000000000000000000000000000000000000005480a0335f1d04855b163f4161cccfa8efbbfeb6a47e5bcd5eb1add240433add8d14aaa0691212a79bd28ce6a41b678ab3cb62b7ae73b0c249ed09eaa3f50bb8378af60b","0x02f8db833018240a845586d5428456bbcb5c830186a08080b880277f50a1448dda96346e7000a223e1f54589f96e366dee877c836e15a005139e9f556030527f2e66e46de1817698c265895c62edbc4808df5dabdc457e39eb59ea16f4297df360505260586070536032607153603260725360ee60735360bc607453601760755360ec607653603f60775360eb6078536064607953603a607a53c001a01eca7512964076914e66a5a3a3b608fe3a068312d387a011ea494bf1dc9e5357a071163472eba4a4ccafe85635cb8dc5a8376b77d9301da97cb623c022c53463a5","0x02f8db833018240b845586d5428456bbcb5c830186a08080b8803d6000593e596000208055ac6000601a552360006005556063605e53607f605f53603f60605360d860615360f460625360f760635360896064536022606553600560665360c1606753600a60685360b1606953606f606a5360ef606b536057606c5360ec606d536037606e53601d606f5360b760705360806071536074607253c080a0ad93d423cd6a79fe77054da64f9e7bf98d089cc6444da847965d8ae426cf99a7a0322c0e765fb85869684698463cc7730f0fd1364ac535497585aa10d6578202b7","0x02f8ef8330182408845586d5428456bbcb5c830186a09476b17a0bfc489801f41d7f438687006b227c8b5480b880f26000605b552457677f207c6b106829d8e3540e7132742338bdf49e955016d1162e30296f11d2f794d060e4527f03854af55342f9dc471bf0f61ef72bb8044e834165e63cd2f8feb20a52249e3f61010452601f61012453606f61012553609761012653600261012753608461012853609f61012953601361012a5360f86101c001a083b0f2718b99abefbd99024d102a08b4f88e6e6eed40dac191746f71904219fda00405cfe0a248b0d005d648ade3eef069cbb5047571236f7ad1eeb0f9cdb327b0","0xf8e8098456bbcb5c830186a0940e1165eba7d740e24e10f94c8db22feae9dfc5f080b88022600060f05575e760006080556000600f55966000605d557f59e99a378c307a09008bbe9fe0953e56cc9a064c0c96f9813d4c77acbb384cc5600e527fce49afc528ef2dc053a940d8d09bc88829d535fbaecda7e6712f9ef1c17fc8e1602e5260b2604e53606a604f536016605053605960515360ad60525360f760535360268360306ca02514693ae2b7a001669ad329d303e3f3d3432792bb4ff08db71f2b1f05a30c08a01cc59d586127910e71915aacd59862015b72ec8148b5080ffdf2c9087e3b6926","0x02f8db833018240c845586d5428456bbcb5c830186a08080b8803d556000604f553a6000607f556000603255159a1460b10c7f06041a48171253b7a759a1a28f2994b571966cd677c5d29808fcd6fb85c173386021527f5476a556be93da1be389e41d383ad4471c80d39639055ea3e3b119faaf2671816041527f1ff89352908230191ca2cab14a567861d2156a1f389b1f23974b77ee05a0c8c001a0c8ec552bf73709537ac1348dbe4df205383368b37b14b1bcb5b62566536c84dda00b3ecb73291bca3e86df6da74279eb6d6780b9531479530e04b1d2c86e01df25","0x01f8ea833018240d8456bbcb5c830186a094000000000000000000000000000000000000000080b8806000609755c7aa4e8e60006077556d858e4f6000600a55626000603d55f4600060fe557fad2bc7b0e139add57a9bda0df24c44a3c31975c27559bf36f31f440ba43ea145606a527ff9201a95ca1b5cd9695a0f81b7324917ed210498e4da028501b4cc02ec63f564608a527f1bfe19e4625048c3e0e878f28a376ebbac42aa27c001a016574afedf00044c36bb39885a2db959a07c9f4d03210f46ea918bca7bac086da007368ca7b6a6cf684fcb91fe0fddd0941ef2853e6d1d077b7594d8479da63f32","0x02f8db8330182408845586d5428456bbcb5c830186a08080b88060ad606a53604a606b5360b8606c53603a606d536f031d600060e555d96000603f55e560006021557fce62e45131a3d2bcc631e262de4439e0fb71745d2731b3fda7c4bc34f6d6a39660e7527f87329e9ebfeb648136472091e5ee5185e24bbb71d413a06d5a497fe822f4c38a610107527f6efa23f8138a05e853e7003d3291c001a03554f1787dd33869e19e42455d6e805de70cd738b8ee4be1a9e1e4cb164d7bc5a077eaf2b2b5a482849639d86cdd743a1c9579f143e001f1be0279b10e46db8dd4","0x02f8ef833018240b845586d5428456bbcb5c830186a0944c9f9564ccc17f5b6f2ae46b28068b4a217f823c80b880156000605d554f742b48600060a855600060fc557f8fb4a628c3daf468c5dbe00049c4e4498efec76ac16c542157cb475da92a79346058527f0e9eeda391df338bf8c354a5a2363390d86d2bca14d3cc8f21b101435cb66c2c60785260ff609853607d6099536053609a5360b4609b5360fa609c5360c9609d53603d609e5360c001a05a51561720d8ad8a180880b1bcf196c02879676aadaa41f56ec3d0bc51e0571ea04ef8921784b8b86ad75da53b0de9ef63d80298233c77281745b9388749c49780","0x01f8ea833018240c8456bbcb5c830186a094000000000000000000000000000000000000000080b8809a603760925360e56093536062609453600c6095536037609653601f60975360d8609853603660995360eb609a536069609b53605c609c536089609d53601d609e53601f609f53609960a05360ce60a15360a660a253603760a353606b60a453602f60a553607e60a653607c60a753604160a85360ba60a95360e860aa53ff60c001a01b8bebfdec1c5bcbc37b2d93b13335b61ade3a8abfa2911d6d38d78f5ca4d3bda0757febf26deb645d5c07c2e4827e0f41433f62dbd2200334a04b26dfa3e27b55","0x02f8ef8330182407845586d5428456bbcb5c830186a0944cf3ec20266d4b889cfc14e95ad61dea0dd3f96380b8803c4b600060b9556000600f556000600f556000601c55600060bc55437fe3d6825f2c052db3316a7c4875028f053b3453c46ca452ab4afb9a286d731ebe600c527fd482bc94b97823b50eba1f0356ffe8ff74b201cfa924dd37ec508ea5523f3769602c527fa4f50fe7435096780e481d8c2700e2ff9c12762d16e1c96510993ec001a0875684b9387e52671c0b0017725a16ba225e6946978d4f7fd54ec2f06ca0cc6aa059035d6c3d217e234b142966b0b6f8a9fac57ce38be58a985096a800ada9cf3d","0x02f8db8330182406845586d5428456bbcb5c830186a08080b880732f18600060e0550b7f75b16b481c6b1dd736fe30df4bb30705f381193ca9b486537500150e1aa8c9ee607052605260905360b1609153601760925360dc60935360e0609453602260955360ff6096536068609753600f60985360366099536062609a536078609b5360f2609c536051609d536038609e5360fd609f5360ed60c001a0f079981b2be325c83e44feced61619d0ad4eeb2e485777a500490643964863a9a025a39902e09633ceecf60f0b0e08d86596e148bb13c73fa5a53a2239417b2647","0xf8d4078456bbcb5c830186a08080b88060006022556000603955abdf600060d9557fe83144d5f4a6372e3a82b8e9f19fab8023974acd28cc161c28017fad35e99c896045527f216ca860f74bcdb612d79573b032377822b0c0976c9bfae3a1fac6513af7f9dd6065527f556562b2142c9e86dc5019119735c7ea6211c48a86e75fc637416f52021e3c636085527f9f268360306ba0f39cb80104b6aa1b5feebe53c384b2e81358ca0ef529a796f2d38f1ed76794c4a00ec9c5b07622759f5e832efe7d38b2f80397d6b574aada8b7a33e27acd8c8f1c","0x02f8ef833018240c845586d5428456bbcb5c830186a0947601ffff9c04946a74de64786b91ca3423e5308a80b8803e600060d85579da9b6000600255600060ce55bf497f77c1dc9ba62d9e51bbd8e683b560f4ebe19c05737099604d3bdd471915fd7b7160a4527fd61ed678ab1ff0ded6469fb135d3b8b8df91966e81126c9babd55d51502abfaf60c4527fdddcf24b615002d2ed63952b4a7c417581f166ae8d3bbda0d58c5a1cbe4ccbfb60e4c001a06090eefc98851ed0849e9a368c28f06f81357942c37afa2dd3e412c5993e6b9ba072ce4ab10a8e1f56e92222a5f8735b13a622f3a0f8e66da96a02115fa628f02e","0x01f8d6833018240d8456bbcb5c830186a08080b8806000603d557f7aa687bf187dca6d21a1c44c5d68a17fe7de9032de38ed96a523c00f8734f60160b4527fa039891cba9eaad46b56f396edc5b5891629066eb2a56b5f49eb83ce1002c0e160d4527f30067aa2a7e0ffe28775ac8c581e693bb16a44c934bfaea5106af6154292d40b60f4527f868288ef2a5981245a78af293945c080a095e64c8d9cc2b26f2698bdb5baebe31cd296050c2861580b2a2aa4c1c7765369a065a840040ff73ace06f2d1180a18d5e5cdffe42084abebf94edcdd133054a8bb","0x02f8ef833018240c845586d5428456bbcb5c830186a094dee9803cf06dc9cf488f1b315ed8f69ba1a11caf80b880600060e255081bc7609260e753605f60e85360f360e95360ed60ea53603460eb53607060ec53606760ed53606260ee5360d860ef53601660f053601860f153605d60f253606160f353602560f453609860f553603660f653604460f753604660f853603560f953606960fa5360e760fb5360d960fc53604c60fd5360cf60fe53c001a0ed6be599e5c6175552b59feb700584ba66d1ae65cb96e5077564a785dcceee6ea05d1dbb6b72c5968447d7fb6c3e67c6c001f7a76da1a1aa9bef26540f8f6ba0dc","0x02f8ef8330182408845586d5428456bbcb5c830186a094ff13380aa959a62874c609d1eb867f78f6da4bee80b8800b84609d60fe53606f60ff53604c61010053605a6101015360796101025360a46101035360ce61010453607061010553605c61010653600261010753606961010853601e61010953602261010a53605561010b5360a661010c53604f61010d5360d361010e5360b161010f53605c6101105360aa6101115360cd61011253603fc001a073eeb6fd9b3995841a9c31a3f1e3b72e2ae580c60ec628675fd41dee2c516222a0323b71918f038e8d1f07fd88450258f5a74dfffa31258eba9dfac1646d206c85","0xf8d4098456bbcb5c830186a08080b8807f40af0f8ccf14c7b5f6ef27a4272f88ce349054583d4d74c4d3a316d7e6480fe860e4527f77ead30c16dd02fa0683f3fdbfb2a76f3c248a7ef8197c85040c3c20bed6f3b9610104527f1465e5109cc4c73296c5c030d5175ce06d1c8729efddb3f359f466b1d973d150610124527fb0a3fd842316b4f7608243e38bf21500608360306ca0dde24ecee4795b1889be7aa30f6e01bdb6da1823d38d5c88a926ee0dc0900543a071cd82efd769b05791dccd26ac61dd8a20518704f83201bf6eb315c5196c9455","0x02f8ef833018240a845586d5428456bbcb5c830186a094fe12d940ca2e96845f4fc81ee239072b725cc59580b8807f50afc5c106ad281e404a583abd770866868b15f34997e7b38ce1bbe319a2f0f86017527ff31a339cc77ab519ff113e13198649c131e28a70a4296cd8ee63d80f09b772eb6037527f0c318fd749ed7b9c3bf91f7df35a39691b14ca062169f967cd4e3287629c96f060575260cb6077536075607853603c607953600f607a53c001a01f3ad45f6a566be7238b7fdb078915c6f31f6dff0ccf9ae1e2bfed9e5fefde40a06306312780fbe8812080b761fce6a839e292b94c61ca371fa61ece368fd9e426","0x02f8ef833018240a845586d5428456bbcb5c830186a09424d622f2386220bb0bef034da4554a7d39b6850a80b8803f60006357f7ea43557f4d82e07caced803bf7cf024f27b7e74ac4ad098fe52d2bb2d8ec734580b4a71360de527f493bc6675e5200418dc54172fb01db534f2d5f6d5bebc946c9b2277afbe9365060fe527f24109a8dbc2b1f0fe4cfa70bfdc9d75b57768dc5ac54526a4e914bb632670bbc61011e527f356dcc70d4b18cc889c080a0ade8e27d56b628bd8843e185c9d5a919bd99bdd86e58d2273d61c2d32cb83a81a0171e6a455788fb502462445ae29777a9e2fac5b8dcdaf1a61615cfa06d0a62d8","0x01f8ea833018240b8456bbcb5c830186a0948bdc941957f6cce7326bcc795a30b490ef33720580b8800f7fd4ad24bb62ce612ef7721150db17021ce259a6de270a1718a18bed721b6f0e7c6089527f25f52abf946ec17b3fc7b13a209edf2956b82b35f12f562691f4938e4e9e57f160a9527fdf23648e8de8e3c3dcdce0f18694c2daed2069aea1a17ad653974d55e74689e060c9527f7c070a51f76f1d94ec641ea1134bb1a1d5f5c080a0b82bdf36235ed4586a6c053a7103f966da6a60c6042e71c802148d7a7439c35aa04dfe0403f180a70bb3fb829d246db612725e153803793837502b9a83dbdc5818","0x02f901378330182406845586d5428456bbcb5c830186a08080b8806000604a5568e4600060915510600060e45560ef601a53606b601b5360e1601c5360a4601d5360f8601e53607f601f5360b160205360de60215360a060225360f460235360146024536021602553600060265360b160275360a360285360b06029536041602a5360a2602b53606c602c53606b602d536096602e536011602f53f85bf8599463425cd368268e80084b4ba27b2bc1d88dad650bf842a0000000000000000000000000000000000000000000000000000000000000004aa000000000000000000000000000000000000000000000000000000000000000e480a0ac73f1a00f068eff47a415f00abe56a67668e98d731d506beabeaefa62a4bddea03078cebbbd92256e205fbdf28d7fd941a81dcfb5df177e62bf6433980857c4c2","0x02f8ef833018240a845586d5428456bbcb5c830186a094b02a2eda1b317fbd16760128836b0ac59b560e9d80b880600060fa55a07f9b1d0e42fb3eae786d2b8837f948e0d9e46b850b6747a9d61d0f6b96a0d9fdc560525260f96072536011607353604e60745360a66075536095607653603560775360e0607853600860795360ed607a53604c607b536001607c536015607d536010607e5360db607f537fadf0bee738b165f70eab02a0b5a16ac080a08ec8648e257894e29abc641255ad36bea72a72b3ddbd7f591bd5f6a8dbab17a1a039c44f736800207e4eb9a68b075c9854fb4e060fcbb6b699aec557d0d81e3fa6","0x01f8d6833018240b8456bbcb5c830186a08080b8807f5026bf06889311a546cb1178c6bc32c41cef803ed6306d41300c239ef6c3a492604d527f67b7e44308d618a8256e6c0bdd3dbb643da7d833a551f90e4884f8478aa2c9ef606d527f9a81d8e137aed73757af423ba8d8df80bb9ca3fc67e318a969b3239876058286608d527feeb4526dc8947003afe246d735cc62f4e1f7f3c001a0dd747d41094f6be99d53867c6806c6cb00aeb1049e02f3393c929c52c735c0d9a0193907fc3618efe9a91f236c5e81b91770cfcd3b8c763e1b534e4b12a3d67687","0x02f8ef8330182409845586d5428456bbcb5c830186a094800d4d0d7cd6a29c5f1dc5f3451994c89439b83f80b880600060885560006045553d6000593e596000208055a8eb3d6000593e596000208055600060ba5560006024556000605c55d67f446086cc8bd5b512074a94b822ca6a66465669648244b0ced25743522df02dc66000527f261daaaee734fa40e6e56c02d1f09571e1fefdcf42e17b3adce7af5811ac44816020527f228b44b172c001a0f493ff40ed1a6b4ce2975614865137974dd9037d931a9d69fa813ed531aaf896a042e31cd3493970d5b186dfdad8f63b96098a515dd541942a2b3e6e80cd1ef7c2","0xf8e80a8456bbcb5c830186a094000000000000000000000000000000000000000080b880600060e055d189d460ea605d5360fb605e5360b3605f5360e1606053604a60615360fc60625360ba60635360b8606453605960655360706066536073606753605260685360a7606953602f606a536064606b53602d606c53602e606d536024606e53607b606f536017607053602660715360696072536025607353605b6074538360306ca077891ac6d2ddfcb9efc6878f33f49625005587501865a5536e646f4f69ba0041a07e65cb9b1aae8d10cfb6d498b85b1f10ee9e4e13dda14c08c494a108cad5b59d","0x02f8ef8330182409845586d5428456bbcb5c830186a094b02a2eda1b317fbd16760128836b0ac59b560e9d80b88060006085553585600060b75553600060d0556000609a557fbea96b1885740544f7f9d69cdb9d119060ada0cd1b3ae28fb0b0af6903770d2460d5527f8aaf4b4465c837386293374bb7b38189ad32276f4a84e29c0d640046fb1809d460f5527fe47bb5e92ffa6f396960896872dfb11419789dda6273d1759dbb556290d3e68dc001a0f8ae4c86d6c8ae081396fd25782190aa6711fa39e7dde149cb7800ec1060695ca011ac5337f0b32b30ea5a5cbaddc5a41cd663e66179003382707a664ab5887a25","0x01f8d6833018240a8456bbcb5c830186a08080b8807f01a576ea7494cf5adb058b04bc27fcf5eec5999b3501015a959f20434e4d51ad60e0527fe371835743a911bcab093b1eb62b2d96a84cb7f44a150758d60017fe2fc68dbf610100527fdc3a7975f6ce51697730cdb8f82dee967490ddefe4c6bcf2aa92c48541b88174610120527fed70f14b47c85cd5df07b40de38381d275c001a026bf872fd7bea7e252c394197a4ec0027207f23db203af5ddf404c9da0f87ca3a02e9dffd55d4d6455859b6c69a2f6eafa9789c31fb6e3b7d3d9c53b3f39bd0385","0x02f8ef8330182408845586d5428456bbcb5c830186a094db3975f2050f1397898548bc9cdd29c947f8baa080b8803d6000593e596000208055dfac60006005557fae031ce2ac651edbcf4e458e56bba5b9ef6c05a9bdd6f9e98674a74bf8b78bca606b527f63009afa59d021571e7a968f0c883822c4ae7ed6bd4bdc164ea78816d53b9850608b527f6d5eaef3effe9806b086f51440c398d3f8b46069a1717a545fdc09032075aa0360ab527fc2c001a0090a51e36f5bdc91b8310405d60e2db27bed918b2d304d785972af80d287b443a0712a8a714ddc62db3a0490b4ec8f897ad18416e4c5dff3789c8cacda53dc3be8","0x01f8d683301824098456bbcb5c830186a08080b88060006009556000603955600060d255c17f58c87a6dfe9638b9eec98699b6a802f28409f3b1c29e2410e47a46476ac5946d6013527ff683f842fd06c60e8a631e13a60e62fcf2b07188b244dc61e8e747a6518b400b6033527fca84d485ccca38d51346cb78800ae64e5498fb464b39db95a8d4d74edc5e14576053527f3c7f1fc001a07c2fc5d616ed77a395891734856c876cc15b5a0c509e51b2d95c1544199aad10a071f7da6a007ec477a8862899b9390678382ad2c08836eb46a54ade6a3b401229","0x02f8ef8330182406845586d5428456bbcb5c830186a094000000000000000000000000000000000000000080b8806000606d557f249b9436ec85c95248eae8d5040e3013c52ab493bb27aeaaf4f71affec73ed0660c3527fb6459347c5a7a3afebc81d309f62e0e9ceab02bfee500f641cabcb57af61437160e3527f4116d5a0997af83bcf9b5864fedaae3c185ab6078d9e7494dc2ed8a48f24f335610103527fa25e193e51a799ac75d1102e8bc001a0cf32fe994f576d05611ba931abb1e21f89eea0bc62911a192a4244b8ff486daea05f8ebfc6c021e2c8fcd317637e652164cf1276d562db56b026ab5b9552319311","0x02f8ef8330182408845586d5428456bbcb5c830186a094b02a2eda1b317fbd16760128836b0ac59b560e9d80b8807f3a688163abd3da0f92b77d0cbe67ed412f646d297c21d60ce3dfb12c7ab711a8607c527f53435e7ecf8f826a40479769885916cf11189b837e3906f757386dfb4eafd9f2609c527faa31f84ee8feeb0ad3bde42fc90d107ea6d3700100272652264eaa6e7dd58de360bc52605460dc5360b260dd53917f64115a4dbd26bdd8c001a05f3f175c6dc7b6e28680b9a116738fcc10922d312a3c70accf9f1dd0a89ddd9ca012c2756e00b92c84c5265c6c5d633ea436c890c974888d2930dc0fb425760ca5","0x02f8ef8330182407845586d5428456bbcb5c830186a0941edb869204b2c116af15f66512accce20812652580b8806000601d55346fc4aad379600060d1553d6000593e5960002080557fede542cf837059763becfbf2bdf6c9e3e6b43fe55e73aafcfa9f2b61d8fc1c7360ba527f6d2152d16e29c835f38b3bda592d9a37deba0739317b48b32367f130e61cbdf460da527fcb8c6c953c9c71bb52e00af94380d8c50ac31d5196c6494c499d9321c080a0b1e750dcd622c5e428e9c438ad63ef8a806eb00d4f76ff8c53e18d23150a5d41a048a434739a5a2f06676ad47e40b4fa012f524ca89735107af1fff611e93a8423","0x02f8db833018240c845586d5428456bbcb5c830186a08080b8806000604a55d160006028557f75b932dc64a723fc8fdd8e3fe6ee65e8bfacaaa4b7935a5f3586bbc052bfa6806052527f1ada94757c35adcb79ec727a741028f697c26124098dc5514286c806c7872fa1607252609e60925360f3609353600f609453602160955360596096536058609753606e60985360b860995360b9609a53c001a0e1255309d27eae4f9140dbb25ed344954b50b59307c25f5c77c0467a8f70d584a02e9b0633e26b3ee14ec5f4d9322b388d753e579b69bdcc49980b7d3c65953cc3","0x02f8db833018240b845586d5428456bbcb5c830186a08080b880956000604255600060c755256000609755e83c6000601555600060a3557aa0226000607d55337f721bbc07aeb1ff2806d7c16a4167e4afcacb276ddd8e07eb52b4345194a796a3609d527fae7596779f1eb87e8411c0d62fd6e23e1bb5fb0774b47ffbf901ce45dfc0059160bd527fbe14e09882bdc844d9feec4810f5581610c080a034da97a78a7c9051870069af66b26a431f4b24a5c5c20ecbff01efeee5a1fec6a036b0c11673cb529ca6f4f667292f7b93ee93ed1ac9ee414ba552f74027d1ac2f","0x02f8db8330182409845586d5428456bbcb5c830186a08080b880df60006073557f1b606aab0c66e5823deabbbe9f2eeedfc77970a591681549240be81f7408a9b46029527f2dcd40096c3ba8327aa6514171e2d8f71dbe7fb0c91f187243874b766a4206856049527fb6fc819371e8773c693822e4619b86dca3204f1468e3ca11557c0b5f68cbfec76069527f72eaa90ea91cbc00ca565973c4c080a0756bd0abb6b8188687ff6c8d51df2eeb5cad588fe50984b5ab10c3c423e468e8a00c55b1cae52719520c8fdc4f3541e95b9f53e69e398338ece51a4ee49947f4e2","0x02f8ef833018240c845586d5428456bbcb5c830186a094038d0bfd781cc55f0bd251a4a4dbfcddab39d2f880b8806000607755965e6000603c557fbff46df78c66c2a4c32fe36c4f47ce7d499b77da98b47036c0ba02bba58b11d1600b527f03f0481bd2d1bd92fa0b2afa732ce7b31420f3d13c5f1840ed5ebcbe8947edb1602b527f4b6a338c59a9caf266686bbc42318bfb209de7f1da0b32f2c8792c476d3f5646604b527f3a7c1e1a8b2b25c001a083dae848ac93e27f33beb11e3303ea50aacb8c2d94b6c9918a4e22adb2d3ff53a056441506efbe274dae8324db6f3abdd861c369911095011b183bee8907828710","0x02f8ef833018240a845586d5428456bbcb5c830186a094000000000000000000000000000000000000000080b880e27f5c48691f62d7cba881d023625534676c8108483371e414c8bcdaed84cbca868d6062527fd21ddc21c1a974b43a948cdf28fc71091a3c0645342dcaa1fb85e6ce3df534c36082527f1db6564bdf41defc99446da515aaff8f311ac0594a37baa8543963500c47d8d760a2527f359a20a5825267f3396446f13252a7e61d22c080a0de268b3be28c13819690066e53e4da031fc33e4611c906af486524dfa98c6c8ca0327655d890d17327ece7f52f8f7af55607f4b1aaa2968d5f1e8bd37d20f361bc","0x02f8ef833018240d845586d5428456bbcb5c830186a094000000000000000000000000000000000000000080b8806000603d55194e6000607e550760cd6069536022606a536014606b53601b606c53609f606d5360d3606e5360f8606f53600060705360b7607153604660725360e060735369600060b25560bb60e153606260e253602660e353604660e453604560e553601860e65360bf60e75360c360e853607960e953601c60ea5360f460ebc001a0e7c756e72461711581360bccb9d011cddfa205703b10544cb25f04b19d3639aba055eab915c5850e87fe2994ace1c296422aee4cdb8744c99729c91a8c66709e78","0x02f8db833018240d845586d5428456bbcb5c830186a08080b880602960f45360d360f553607a60f65360ce60f75360c060f85360bb60f95360df60fa536000605d55600063f986030255337f902da9940c3ee9ee9ac2259b7eff0c21553ef3d56cfd0a731711b484560768cb6097527f94e4f7f8f665cc4f5326eb6c2907258bab93859c382cd9b4a1881e278639fdbd60b75260d360d75360e5c080a060ee18ba4d536de7d801191fafdbcb01d538ee8d513b3c1b5fae82092bfb9237a01f9d93febcd395459518f8cbcc0271372b2cdb7ab21d704c944371778fc6f62f","0x02f8ee833018240c845586d5428456bbcb5c830186a094b02a2eda1b317fbd16760128836b0ac59b560e9d80b8807fb36102cd7b3e07b8fb27761311b2bd17021b94447d81fee4b3b0c804e44ae4436062527f28487e7ee08f869058252b8854e266228a479573dd61f83e24083d77584ccbc96082527feb4de445313598f797da2950634848390b011a738a90477f7d1281580805382860a2527f585689b11fe968e85c1c68f48088edf7ee9f39c001a0b1a2a6d685ecafe6b45777b591e3f7b70e569c3c513bf55f0c10ea90956370749fbe08401c2ba5ff600b04c6c1e81385a3acd643f1f3b2d8debe317026e4caca","0x02f8ef8330182409845586d5428456bbcb5c830186a0941428d6378f6768407f92153cfbcef4965c1a483880b880fd7f8e28ddd6e650a6d10c87a64c5aa1932c714635d9945e1b37a0f15c23835cb8e76051527f74c6b944e29352c4d0d0a64cafbf952d879e90dccc5aa5c1f5bb9616fc5efe356071527fa485d4b829f1c3d09cbebd351451eaa8bcd0d0eb713ed61fe3257e437b7c2db8609152603f60b153608960b253602b60b35360c260b4c080a02e9f459b710c171ce5ce8a519c732ded562c5a81cdf074224dc5602790815999a011c37d0c3cffb1599d6618834ca003d77fdd4c472734c1b656622dd46866e6e5","0x02f8ef833018240b845586d5428456bbcb5c830186a0943a38a4ef192deed60e9d820e1bae30fdec4ad53080b8807fd4b7193d163b1484e804c1eaa3e4ab2127bef6d4555f84c4a986785e7c6b85c26099527f98ecd16ace88564bc37dbfa383765ea2e0e80b287f207e036794ae01b0bfe56f60b95260ae60d953606460da5360de60db53600060f1557fd5c41aa3cf23bece18649b6b59851394ad1b7cafc94d0ff9edc78dd583988120600652c080a003053f583c3770b170c4514bf25cf38c9275a4ed3ed1e1c174cda7f39df9c48ea04a3d7102a478f9c9f2fda2379d505488c036476fa8ab587e04caf36d38a7cf92","0x02f8ef833018240c845586d5428456bbcb5c830186a0944abf1174d25441f242a362fc830ae0297473c1a780b880341360006065557f42d46119abca236161541ee457aee5c6af672992eb49409b31fe8ea1b263aecd60e0527f44615fd18a298ed04ef3a32e4ddfbdaf2a2c8eef3e1d4df014010da6a25302f9610100527f2d02afa5b0ddc61dbe3f2ff4615a5cfdedbe0afe8d6bbbcaf4748169f85ecc3b610120527f1edd8a6dbfaa665af664c080a0f3f2832a70000545479ed9abbac9ee7806ea8f0c7d1ed69e74a44ba48fc5d78da05f53ed93fae9d067e219837098c282205d9addb3ad8da6536d751fe9f8a59513","0x02f8ef8330182409845586d5428456bbcb5c830186a0940ac949e990b7e26cf35ce4891c292a65a06d7dff80b880786360006077556000609555ff60006021559836f9609b60cd53607f60ce5360f360cf53605860d05360fe60d15360f760d253601b60d35360f460d453604f60d553606560d653602660d753607e60d85360a360d953604f60da53601c60db5360e760dc5360b760dd5360ae60de5360d960df5360e060e053601860e1536018c080a0a213f16b96ad260f3da5e46041ab2766d4e788d624c0bc58e3ea2f894602716aa02392e1eebf61ff3249088a572c11145987d772085a6baf10d54bb8fb4f382c50","0x02f8ef833018240b845586d5428456bbcb5c830186a094351b49fdea12e169c67513418805b8d3cc66905d80b880600060c055600060bb55600060d2554d600060d1556000607355329f7fa4cdf9800ae2ba0424ec3fdd7b41c81df4a2435048ca9ec30ff55f724ff55e5f60de527fad166342c9d4a0016cecd95dee02076800fdece208025df4e5f290a564ce92e460fe527fcac766319baf4a67e9d7aedea7dcf16e647703541249689bdb699ac001a0dca2616ff968457f1acd70402402d1ac4942d715bea71c5766878789295b2ad5a05383942b05a8381610b28cf9687e71b4e49a0d15d61164a0bcbfc32cbb9e030e","0x02f8ef833018240b845586d5428456bbcb5c830186a094b02a2eda1b317fbd16760128836b0ac59b560e9d80b880600060eb557f77bf002a11e6c786578c03f32a7bc063c8827016a59b34c4ab3552d360012cf060db52604960fb5360ba60fc53603160fd5360b860fe53601260ff5360ec6101005360bf61010153601d61010253604561010353606a61010453605f610105537f44bc4e21b4ec1d32eb4f29a047ef3a5623a2797b844243e01dc080a04caa853ec730671ef067dd486dcab6a27b940c956ecb15c4eb9922506b1594cca06b9d41109285b94933b1d5e4684b54c82326ab61a474483d06ce377ca880fd74","0x02f8ef833018240d845586d5428456bbcb5c830186a09426358cf04bb59a1d3b93aee88154c4af235bf7c380b880977f81b700d7645af0bc2ad3c3060bdedff7a68d4079a5ccc229e3ab1f8b4ed172be6021527f1a3d836d0f1593c6b7f729d15d4eabb41a8346214faa4d03ffe4c1627f33620b6041527fe357588040ddab2b007ee939ee4fdcf63ae54b84fad75735491fbe1f7dc23f6760615260ce608153605a60825360c6608353605c6084c001a08af6ac41cc346047a037fda0672e89cd45408aca76bf65af916fc46187a03bd9a02a6c70bfb1208fe266f1736160979e82fb2fc3fa69ebf9ef85d1f9a490f515ad","0x02f8ef8330182409845586d5428456bbcb5c830186a094a2aa4216a22b2db2f5ebf4de77d0307f1862673f80b880091d60006074557ffe053ebc76fccd719876f0ad5b6620f3454e521c9cbb2a7185cd235eb4524bb6609d527fcb9234b240f52614ecf9f67166ffd9d5bdbe70d5aa5189c35373bbb14bdd4af160bd527f411d2b7f8ae81df1f0abbb61f1d87c67645f50571c40d265d37fd7ec8609725060dd52608e60fd53601060fe53605b60c080a0f043bfaf5fedae41e452fbcb99d2d735114377302f611b1d507b06ca8e77b7b7a045b44d0fa7135af9b7f729971faa0933e9e3dc510a1b6f2c91e5bf433f8488f8","0x02f8db833018240c845586d5428456bbcb5c830186a08080b8807f1102d0531c00b1d2827698f50a9b67496fc8f1fc2f3910f5851bcdf804158cb960a9527fbed46fdc49d3c267558f2684b8637c0ba5778ff5e7f14e1a651a5c58b8dc62ad60c9527fe91628a415712eedc5727b8d4d1258a1be19ecf5f403381a51a049e946f967c460e9527f2ef416179e18dbfef1b57e4fe93d8706c104c0c001a021edd4664aef62a81ee2e84a2a2fe0199b6fe22e6d904ed616b5a5f700298a5fa07e68980cc6b4b757108c3389e86de5ce445adca5d8640b71a9b08bb484f2261c","0x02f8ef8330182407845586d5428456bbcb5c830186a0945e8db5cc09b13a78f4ca05f18f13d1e8b526c00280b880045ede7fda555a0514ca2aaebf231de9f761055457427dfdcaa0b5be98efc3218bca32bc60a8527fdb5f8d5a4fe77ca4021f5b10394dbc48663926a48681171ead154d829d0fcbbc60c8527f84c75397118514ceba6e33f9f746ac991b3810b324dbd3b02e9d3aae2ce5c5e760e8527f0b8e79b65188e25278cd99784237d62cc080a0b773f1fecaf03544b05a3c921ecde3391f44dcda658d619606a9ee5937f99da4a0751d99376c61bf1e897d0ef2b9f3f73358c883a71c44446e7061ea5bf864444a","0x02f8ef8330182408845586d5428456bbcb5c830186a094000000000000000000000000000000000000000080b88024027f2cf982b4aa2fb2b18062591b01b9c0c400e2fc5f85b7b1731b1c7beff410272f60c9527f39b564764e003c074632afc451ab56621c8f1bf6e91b1cae6245add9d0f031d360e9527fe0b546297f1d0086a00f29a977284b6d5a57b6859e42c7b1386f3bba2c7daa8e610109527f817703cf765ab03283f1488f42440dcbc080a034f3ea6ca8839a321e00ad7c2e8b30a082ac8beb342607188db90409719a6529a053ebc20fdaa310f9a3c522633a31f33f64f2139800c20658608d81eb5b1ed184","0x02f8ef833018240b845586d5428456bbcb5c830186a09434238b0ef79cb97b13e548d413bd5d258776cef280b88075b16000606655600060dc555f906000601c557f26ea2cbb201eca6dd73505eeafc240c7b43411c0fb6427f019c252e17bbe56466038526012605853607f6059536093605a536077605b53603f605c536001605d53600b605e536071605f53607560605360bc60615360ff6062537fc8e5462da42c1b4c18764c9d6be98b592ec001a092995f40bb94ee7b3360a9a489a44319f54795c46cd004fce93ea9a730227cb7a04ea26928198ff196799bbaf56e00ab6adcbe95f6461d616f396090c52211c58f","0x02f8ef833018240a845586d5428456bbcb5c830186a094540f187763b170ef6457aa71c9ba9c573f5d3b0980b8800c600060b6557f83e61e97099798fc0b9df7102b36f9801daf34b2129ef68b933b045cb67ba7d26009527fa3863a2ba645b6911bd280e4a279d98ea653b49c626f009d7411e02d6ea499936029527f29e1ffe6406f531dc6ca1dfd1a7febb6c0303a1453ed02b98270da58665781246049527f5bebbbd20657009ccd98011390c080a0cc6cf137131ed431b23c6b51e8b88660dfe7c2ffef44ef33306bf386268ba32da0529ef8606ccf71191cb742f763b646f0d3725e8409a182241dc88c64f8efda7c","0x02f9021e8330182480843b9aca0085174876e80083030d40944242424242424242424242424242424242424242893635c9adc5dea00000b901a422895118000000000000000000000000000000000000000000000000000000000000008000000000000000000000000000000000000000000000000000000000000000e000000000000000000000000000000000000000000000000000000000000001202ce48a62460a2d60856e140054ba5840454b823c9ed5fc991ab7aefe9487d38200000000000000000000000000000000000000000000000000000000000000308781f734dcbbb41234a2f6e5b08b6dcd5babec3635011f6eeafc58c6e154d4d07549aaf5cc7fa05f127dc485bd757b4900000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002000e02267460dfed7870106fe6be1131e22ad802fecab489e13d4295980f696e90000000000000000000000000000000000000000000000000000000000000060b47afffc53dcf130939ed6d516cb29f9d9a4631e5d122a0b3b86cdceca6ffe91acad77a02eb78556a1295fca7e08e8630d5e08433f72847f78b62a3913cc83f54c3486650e9d440db7cdbd836ac0b840db814a03cccfd3759e2ea55f929d999fc001a0c5631ecff95d8c7a04cd8c6d033c114904ebdf1ede4dd14e042b295daffc630aa0610ba4ba73a95952d4edce7c17c7801121b4f1f99de60b25d93982dec8c57244"],"withdrawalRequests":[],"withdrawals":[{"address":"0x65d08a056c17ae13370565b04cf77d2afa1cb9fa","amount":"0x1ee","index":"0x7","validatorIndex":"0x0"},{"address":"0x65d08a056c17ae13370565b04cf77d2afa1cb9fa","amount":"0x1ee","index":"0x8","validatorIndex":"0x1"},{"address":"0x65d08a056c17ae13370565b04cf77d2afa1cb9fa","amount":"0x1ee","index":"0x9","validatorIndex":"0x2"},{"address":"0x65d08a056c17ae13370565b04cf77d2afa1cb9fa","amount":"0x1ee","index":"0xa","validatorIndex":"0x3"},{"address":"0x65d08a056c17ae13370565b04cf77d2afa1cb9fa","amount":"0x1ee","index":"0xb","validatorIndex":"0x4"},{"address":"0x65d08a056c17ae13370565b04cf77d2afa1cb9fa","amount":"0x1ee","index":"0xc","validatorIndex":"0x5"},{"address":"0x65d08a056c17ae13370565b04cf77d2afa1cb9fa","amount":"0x1ee","index":"0xd","validatorIndex":"0x6"},{"address":"0x65d08a056c17ae13370565b04cf77d2afa1cb9fa","amount":"0x419c","index":"0xe","validatorIndex":"0x11c"},{"address":"0x65d08a056c17ae13370565b04cf77d2afa1cb9fa","amount":"0x419c","index":"0xf","validatorIndex":"0x11d"},{"address":"0x65d08a056c17ae13370565b04cf77d2afa1cb9fa","amount":"0x419c","index":"0x10","validatorIndex":"0x11e"},{"address":"0x65d08a056c17ae13370565b04cf77d2afa1cb9fa","amount":"0x419c","index":"0x11","validatorIndex":"0x11f"},{"address":"0x65d08a056c17ae13370565b04cf77d2afa1cb9fa","amount":"0x419c","index":"0x12","validatorIndex":"0x120"},{"address":"0x65d08a056c17ae13370565b04cf77d2afa1cb9fa","amount":"0x419c","index":"0x13","validatorIndex":"0x121"},{"address":"0x65d08a056c17ae13370565b04cf77d2afa1cb9fa","amount":"0x419c","index":"0x14","validatorIndex":"0x122"},{"address":"0x65d08a056c17ae13370565b04cf77d2afa1cb9fa","amount":"0x419c","index":"0x15","validatorIndex":"0x123"},{"address":"0x65d08a056c17ae13370565b04cf77d2afa1cb9fa","amount":"0x419c","index":"0x16","validatorIndex":"0x124"}]}"#;

        let payload = serde_json::from_str::<ExecutionPayloadV4>(payload).unwrap();

        let block = try_payload_v4_to_block(
            payload,
            b256!("e15555c7dc99b0a33d57a7b1b7a4453723e86e4106d0ba044157125e350d1b02"),
        )
        .unwrap();

        assert_eq!(
            block.seal_slow().hash(),
            b256!("ba70810536cb83f8b299a9ab555c9d89c72d378d5ac3aaecd8bd2e2a7b479ab2")
        );
    }
}
