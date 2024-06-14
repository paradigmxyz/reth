//! Standalone Conversion Functions for Handling Different Versions of Execution Payloads in
//! Ethereum's Engine

use reth_primitives::{
    constants::{EMPTY_OMMER_ROOT_HASH, MAXIMUM_EXTRA_DATA_SIZE},
    proofs::{self},
    Block, Header, Request, SealedBlock, TransactionSigned, UintTryTo, Withdrawals, B256, U256,
};
use reth_rpc_types::engine::{
    payload::{ExecutionPayloadBodyV1, ExecutionPayloadFieldV2, ExecutionPayloadInputV2},
    ExecutionPayload, ExecutionPayloadV1, ExecutionPayloadV2, ExecutionPayloadV3,
    ExecutionPayloadV4, PayloadError,
};

/// Converts [`ExecutionPayloadV1`] to [Block]
pub fn try_payload_v1_to_block(payload: ExecutionPayloadV1) -> Result<Block, PayloadError> {
    if payload.extra_data.len() > MAXIMUM_EXTRA_DATA_SIZE {
        return Err(PayloadError::ExtraData(payload.extra_data))
    }

    if payload.base_fee_per_gas.is_zero() {
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
        // WARNING: It’s allowed for a base fee in EIP1559 to increase unbounded. We assume that
        // it will fit in an u64. This is not always necessarily true, although it is extremely
        // unlikely not to be the case, a u64 maximum would have 2^64 which equates to 18 ETH per
        // gas.
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

/// Converts [`ExecutionPayloadV2`] to [Block]
pub fn try_payload_v2_to_block(payload: ExecutionPayloadV2) -> Result<Block, PayloadError> {
    // this performs the same conversion as the underlying V1 payload, but calculates the
    // withdrawals root and adds withdrawals
    let mut base_sealed_block = try_payload_v1_to_block(payload.payload_inner)?;
    let withdrawals_root = proofs::calculate_withdrawals_root(&payload.withdrawals);
    base_sealed_block.withdrawals = Some(payload.withdrawals.into());
    base_sealed_block.header.withdrawals_root = Some(withdrawals_root);
    Ok(base_sealed_block)
}

/// Converts [`ExecutionPayloadV3`] to [Block]
pub fn try_payload_v3_to_block(payload: ExecutionPayloadV3) -> Result<Block, PayloadError> {
    // this performs the same conversion as the underlying V2 payload, but inserts the blob gas
    // used and excess blob gas
    let mut base_block = try_payload_v2_to_block(payload.payload_inner)?;

    base_block.header.blob_gas_used = Some(payload.blob_gas_used);
    base_block.header.excess_blob_gas = Some(payload.excess_blob_gas);

    Ok(base_block)
}

/// Converts [`ExecutionPayloadV4`] to [Block]
pub fn try_payload_v4_to_block(payload: ExecutionPayloadV4) -> Result<Block, PayloadError> {
    let ExecutionPayloadV4 { payload_inner, deposit_requests, withdrawal_requests } = payload;
    let mut block = try_payload_v3_to_block(payload_inner)?;

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

/// Converts [`SealedBlock`] to [`ExecutionPayload`]
pub fn block_to_payload(value: SealedBlock) -> (ExecutionPayload, Option<B256>) {
    if value.header.requests_root.is_some() {
        (ExecutionPayload::V4(block_to_payload_v4(value)), None)
    } else if value.header.parent_beacon_block_root.is_some() {
        // block with parent beacon block root: V3
        let (payload, beacon_block_root) = block_to_payload_v3(value);
        (ExecutionPayload::V3(payload), beacon_block_root)
    } else if value.withdrawals.is_some() {
        // block with withdrawals: V2
        (ExecutionPayload::V2(block_to_payload_v2(value)), None)
    } else {
        // otherwise V1
        (ExecutionPayload::V1(block_to_payload_v1(value)), None)
    }
}

/// Converts [`SealedBlock`] to [`ExecutionPayloadV1`]
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

/// Converts [`SealedBlock`] to [`ExecutionPayloadV2`]
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

/// Converts [`SealedBlock`] to [`ExecutionPayloadV3`], and returns the parent beacon block root.
pub fn block_to_payload_v3(value: SealedBlock) -> (ExecutionPayloadV3, Option<B256>) {
    let transactions = value.raw_transactions();

    let parent_beacon_block_root = value.header.parent_beacon_block_root;
    let payload = ExecutionPayloadV3 {
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
    };

    (payload, parent_beacon_block_root)
}

/// Converts [`SealedBlock`] to [`ExecutionPayloadV4`]
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
        payload_inner: block_to_payload_v3(value).0,
    }
}

/// Converts [`SealedBlock`] to [`ExecutionPayloadFieldV2`]
pub fn convert_block_to_payload_field_v2(value: SealedBlock) -> ExecutionPayloadFieldV2 {
    // if there are withdrawals, return V2
    if value.withdrawals.is_some() {
        ExecutionPayloadFieldV2::V2(block_to_payload_v2(value))
    } else {
        ExecutionPayloadFieldV2::V1(block_to_payload_v1(value))
    }
}

/// Converts [`ExecutionPayloadFieldV2`] to [`ExecutionPayload`]
pub fn convert_payload_field_v2_to_payload(value: ExecutionPayloadFieldV2) -> ExecutionPayload {
    match value {
        ExecutionPayloadFieldV2::V1(payload) => ExecutionPayload::V1(payload),
        ExecutionPayloadFieldV2::V2(payload) => ExecutionPayload::V2(payload),
    }
}

/// Converts [`ExecutionPayloadV2`] to [`ExecutionPayloadInputV2`].
///
/// An [`ExecutionPayloadInputV2`] should have a [`Some`] withdrawals field if shanghai is active,
/// otherwise the withdrawals field should be [`None`], so the `is_shanghai_active` argument is
/// provided which will either:
/// - include the withdrawals field as [`Some`] if true
/// - set the withdrawals field to [`None`] if false
pub fn convert_payload_v2_to_payload_input_v2(
    value: ExecutionPayloadV2,
    is_shanghai_active: bool,
) -> ExecutionPayloadInputV2 {
    ExecutionPayloadInputV2 {
        execution_payload: value.payload_inner,
        withdrawals: is_shanghai_active.then_some(value.withdrawals),
    }
}

/// Converts [`ExecutionPayloadInputV2`] to [`ExecutionPayload`]
pub fn convert_payload_input_v2_to_payload(value: ExecutionPayloadInputV2) -> ExecutionPayload {
    match value.withdrawals {
        Some(withdrawals) => ExecutionPayload::V2(ExecutionPayloadV2 {
            payload_inner: value.execution_payload,
            withdrawals,
        }),
        None => ExecutionPayload::V1(value.execution_payload),
    }
}

/// Converts [`SealedBlock`] to [`ExecutionPayloadInputV2`]
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
    let mut base_payload = match value {
        ExecutionPayload::V1(payload) => try_payload_v1_to_block(payload)?,
        ExecutionPayload::V2(payload) => try_payload_v2_to_block(payload)?,
        ExecutionPayload::V3(payload) => try_payload_v3_to_block(payload)?,
        ExecutionPayload::V4(payload) => try_payload_v4_to_block(payload)?,
    };

    base_payload.header.parent_beacon_block_root = parent_beacon_block_root;

    Ok(base_payload)
}

/// Tries to create a new block from the given payload and optional parent beacon block root.
///
/// NOTE: Empty ommers, nonce and difficulty values are validated upon computing block hash and
/// comparing the value with `payload.block_hash`.
///
/// Uses [`try_into_block`] to convert from the [`ExecutionPayload`] to [Block] and seals the block
/// with its hash.
///
/// Uses [`validate_block_hash`] to validate the payload block hash and ultimately return the
/// [`SealedBlock`].
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
/// [`SealedBlock`].
///
/// If the provided block hash does not match the block hash computed from the provided block, this
/// returns [`PayloadError::BlockHash`].
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

/// Converts [Block] to [`ExecutionPayloadBodyV1`]
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

/// Transforms a [`SealedBlock`] into a [`ExecutionPayloadV1`]
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
    use reth_primitives::{b256, hex, Bytes, U256};
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

        let mut block = try_payload_v3_to_block(new_payload.clone()).unwrap();

        // this newPayload came with a parent beacon block root, we need to manually insert it
        // before hashing
        let parent_beacon_block_root =
            b256!("531cd53b8e68deef0ea65edfa3cda927a846c307b0907657af34bc3f313b5871");
        block.header.parent_beacon_block_root = Some(parent_beacon_block_root);

        let converted_payload = block_to_payload_v3(block.seal_slow());

        // ensure the payloads are the same
        assert_eq!((new_payload, Some(parent_beacon_block_root)), converted_payload);
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

        let _block = try_payload_v3_to_block(new_payload)
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
      "blockHash": "0x86eeb2a4b656499f313b601e1dcaedfeacccab27131b6d4ea99bc69a57607f7d",
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

        let payload = serde_json::from_str::<ExecutionPayloadV4>(s).unwrap();
        let mut block = try_payload_v4_to_block(payload).unwrap();
        block.header.parent_beacon_block_root =
            Some(b256!("d9851db05fa63593f75e2b12c4bba9f47740613ca57da3b523a381b8c27f3297"));
        let hash = block.seal_slow().hash();
        assert_eq!(hash, b256!("86eeb2a4b656499f313b601e1dcaedfeacccab27131b6d4ea99bc69a57607f7d"))
    }
}
