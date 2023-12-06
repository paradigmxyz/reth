//! Some payload tests

use alloy_rlp::{Decodable, Error as RlpError};
use assert_matches::assert_matches;
use reth_interfaces::test_utils::generators::{
    self, random_block, random_block_range, random_header, Rng,
};
use reth_primitives::{
    bytes::{Bytes, BytesMut},
    proofs, Block, SealedBlock, TransactionSigned, B256, U256,
};
use reth_rpc_types::engine::{
    ExecutionPayload, ExecutionPayloadBodyV1, ExecutionPayloadV1, PayloadError,
};
use reth_rpc_types_compat::engine::payload::{
    convert_standalone_withdraw_to_withdrawal, convert_to_payload_body_v1, try_block_to_payload,
    try_block_to_payload_v1, try_into_sealed_block, try_payload_v1_to_block,
};

fn transform_block<F: FnOnce(Block) -> Block>(src: SealedBlock, f: F) -> ExecutionPayload {
    let unsealed = src.unseal();
    let mut transformed: Block = f(unsealed);
    // Recalculate roots
    transformed.header.transactions_root = proofs::calculate_transaction_root(&transformed.body);
    transformed.header.ommers_hash = proofs::calculate_ommers_root(&transformed.ommers);
    try_block_to_payload(SealedBlock {
        header: transformed.header.seal_slow(),
        body: transformed.body,
        ommers: transformed.ommers,
        withdrawals: transformed.withdrawals,
    })
}

#[test]
fn payload_body_roundtrip() {
    let mut rng = generators::rng();
    for block in random_block_range(&mut rng, 0..=99, B256::default(), 0..2) {
        let unsealed = block.clone().unseal();
        let payload_body: ExecutionPayloadBodyV1 = convert_to_payload_body_v1(unsealed);

        assert_eq!(
            Ok(block.body),
            payload_body
                .transactions
                .iter()
                .map(|x| TransactionSigned::decode(&mut &x[..]))
                .collect::<Result<Vec<_>, _>>(),
        );
        let withdraw = payload_body.withdrawals.map(|withdrawals| {
            withdrawals
                .into_iter()
                .map(convert_standalone_withdraw_to_withdrawal)
                .collect::<Vec<_>>()
        });
        assert_eq!(block.withdrawals, withdraw);
    }
}

#[test]
fn payload_validation() {
    let mut rng = generators::rng();
    let parent = rng.gen();
    let block = random_block(&mut rng, 100, Some(parent), Some(3), Some(0));

    // Valid extra data
    let block_with_valid_extra_data = transform_block(block.clone(), |mut b| {
        b.header.extra_data = BytesMut::zeroed(32).freeze().into();
        b
    });

    assert_matches!(try_into_sealed_block(block_with_valid_extra_data, None), Ok(_));

    // Invalid extra data
    let block_with_invalid_extra_data: Bytes = BytesMut::zeroed(33).freeze();
    let invalid_extra_data_block = transform_block(block.clone(), |mut b| {
        b.header.extra_data = block_with_invalid_extra_data.clone().into();
        b
    });
    assert_matches!(

        try_into_sealed_block(invalid_extra_data_block,None),
        Err(PayloadError::ExtraData(data)) if data == block_with_invalid_extra_data
    );

    // Zero base fee
    let block_with_zero_base_fee = transform_block(block.clone(), |mut b| {
        b.header.base_fee_per_gas = Some(0);
        b
    });
    assert_matches!(

        try_into_sealed_block(block_with_zero_base_fee,None),
        Err(PayloadError::BaseFee(val)) if val.is_zero()
    );

    // Invalid encoded transactions
    let mut payload_with_invalid_txs: ExecutionPayloadV1 = try_block_to_payload_v1(block.clone());

    payload_with_invalid_txs.transactions.iter_mut().for_each(|tx| {
        *tx = Bytes::new().into();
    });
    let payload_with_invalid_txs = try_payload_v1_to_block(payload_with_invalid_txs);
    assert_matches!(payload_with_invalid_txs, Err(PayloadError::Decode(RlpError::InputTooShort)));

    // Non empty ommers
    let block_with_ommers = transform_block(block.clone(), |mut b| {
        b.ommers.push(random_header(&mut rng, 100, None).unseal());
        b
    });
    assert_matches!(
        try_into_sealed_block(block_with_ommers.clone(),None),

        Err(PayloadError::BlockHash { consensus, .. })
            if consensus == block_with_ommers.block_hash()
    );

    // None zero difficulty
    let block_with_difficulty = transform_block(block.clone(), |mut b| {
        b.header.difficulty = U256::from(1);
        b
    });
    assert_matches!(
        try_into_sealed_block(block_with_difficulty.clone(),None),
        Err(PayloadError::BlockHash { consensus, .. }) if consensus == block_with_difficulty.block_hash()

    );

    // None zero nonce
    let block_with_nonce = transform_block(block.clone(), |mut b| {
        b.header.nonce = 1;
        b
    });
    assert_matches!(
        try_into_sealed_block(block_with_nonce.clone(),None),
        Err(PayloadError::BlockHash { consensus, .. }) if consensus == block_with_nonce.block_hash()

    );

    // Valid block
    let valid_block = block;
    assert_matches!(TryInto::<SealedBlock>::try_into(valid_block), Ok(_));
}
