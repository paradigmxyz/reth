//! Some payload tests

use alloy_eips::eip4895::Withdrawals;
use alloy_primitives::Bytes;
use alloy_rlp::{Decodable, Error as RlpError};
use alloy_rpc_types_engine::{
    ExecutionPayload, ExecutionPayloadBodyV1, ExecutionPayloadSidecar, ExecutionPayloadV1,
    PayloadError,
};
use assert_matches::assert_matches;
use reth_primitives::{Block, SealedBlock, SealedHeader, TransactionSigned};
use reth_primitives_traits::proofs;
use reth_rpc_types_compat::engine::payload::{block_to_payload, block_to_payload_v1};
use reth_testing_utils::generators::{
    self, random_block, random_block_range, BlockParams, BlockRangeParams, Rng,
};

fn transform_block<F: FnOnce(Block) -> Block>(src: SealedBlock, f: F) -> ExecutionPayload {
    let unsealed = src.into_block();
    let mut transformed: Block = f(unsealed);
    // Recalculate roots
    transformed.header.transactions_root =
        proofs::calculate_transaction_root(&transformed.body.transactions);
    transformed.header.ommers_hash = proofs::calculate_ommers_root(&transformed.body.ommers);
    block_to_payload(SealedBlock::from_sealed_parts(
        SealedHeader::seal_slow(transformed.header),
        transformed.body,
    ))
    .0
}

#[test]
fn payload_body_roundtrip() {
    let mut rng = generators::rng();
    for block in random_block_range(
        &mut rng,
        0..=99,
        BlockRangeParams { tx_count: 0..2, ..Default::default() },
    ) {
        let payload_body: ExecutionPayloadBodyV1 =
            ExecutionPayloadBodyV1::from_block(block.clone().into_block());

        assert_eq!(
            Ok(block.body().transactions.clone()),
            payload_body
                .transactions
                .iter()
                .map(|x| TransactionSigned::decode(&mut &x[..]))
                .collect::<Result<Vec<_>, _>>(),
        );
        let withdraw = payload_body.withdrawals.map(Withdrawals::new);
        assert_eq!(block.body().withdrawals.clone(), withdraw);
    }
}

#[test]
fn payload_validation_conversion() {
    let mut rng = generators::rng();
    let parent = rng.gen();
    let block = random_block(
        &mut rng,
        100,
        BlockParams {
            parent: Some(parent),
            tx_count: Some(3),
            ommers_count: Some(0),
            ..Default::default()
        },
    );

    // Valid extra data
    let block_with_valid_extra_data = transform_block(block.clone(), |mut b| {
        b.header.extra_data = Bytes::from_static(&[0; 32]);
        b
    });

    assert_matches!(
        block_with_valid_extra_data
            .try_into_block_with_sidecar::<TransactionSigned>(&ExecutionPayloadSidecar::none()),
        Ok(_)
    );

    // Invalid extra data
    let block_with_invalid_extra_data = Bytes::from_static(&[0; 33]);
    let invalid_extra_data_block = transform_block(block.clone(), |mut b| {
        b.header.extra_data = block_with_invalid_extra_data.clone();
        b
    });
    assert_matches!(
        invalid_extra_data_block.try_into_block_with_sidecar::<TransactionSigned>(&ExecutionPayloadSidecar::none()),
        Err(PayloadError::ExtraData(data)) if data == block_with_invalid_extra_data
    );

    // Zero base fee
    let block_with_zero_base_fee = transform_block(block.clone(), |mut b| {
        b.header.base_fee_per_gas = Some(0);
        b
    });
    assert_matches!(
        block_with_zero_base_fee.try_into_block_with_sidecar::<TransactionSigned>(&ExecutionPayloadSidecar::none()),
        Err(PayloadError::BaseFee(val)) if val.is_zero()
    );

    // Invalid encoded transactions
    let mut payload_with_invalid_txs: ExecutionPayloadV1 = block_to_payload_v1(block);

    payload_with_invalid_txs.transactions.iter_mut().for_each(|tx| {
        *tx = Bytes::new();
    });
    let payload_with_invalid_txs = payload_with_invalid_txs.try_into_block::<TransactionSigned>();
    assert_matches!(payload_with_invalid_txs, Err(PayloadError::Decode(RlpError::InputTooShort)));
}
