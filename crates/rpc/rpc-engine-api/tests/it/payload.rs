//! Some payload tests

use assert_matches::assert_matches;
use reth_interfaces::test_utils::generators::{
    self, random_block, random_block_range, random_header,
};
use reth_primitives::{
    bytes::{Bytes, BytesMut},
    proofs::{self},
    Block, SealedBlock, TransactionSigned, H256, U256,
};
use reth_rlp::{Decodable, DecodeError};
use reth_rpc_types::engine::{
    ExecutionPayload, ExecutionPayloadBodyV1, ExecutionPayloadV1, PayloadError,
};

fn transform_block<F: FnOnce(Block) -> Block>(src: SealedBlock, f: F) -> ExecutionPayload {
    let unsealed = src.unseal();
    let mut transformed: Block = f(unsealed);
    // Recalculate roots
    transformed.header.transactions_root = proofs::calculate_transaction_root(&transformed.body);
    transformed.header.ommers_hash = proofs::calculate_ommers_root(&transformed.ommers);
    SealedBlock {
        header: transformed.header.seal_slow(),
        body: transformed.body,
        ommers: transformed.ommers,
        withdrawals: transformed.withdrawals,
    }
    .into()
}

#[test]
fn payload_body_roundtrip() {
    let mut rng = generators::rng();
    for block in random_block_range(&mut rng, 0..=99, H256::default(), 0..2) {
        let unsealed = block.clone().unseal();
        let payload_body: ExecutionPayloadBodyV1 = unsealed.into();

        assert_eq!(
            Ok(block.body),
            payload_body
                .transactions
                .iter()
                .map(|x| TransactionSigned::decode(&mut &x[..]))
                .collect::<Result<Vec<_>, _>>(),
        );

        assert_eq!(block.withdrawals, payload_body.withdrawals);
    }
}

#[test]
fn payload_validation() {
    let mut rng = generators::rng();
    let block = random_block(&mut rng, 100, Some(H256::random()), Some(3), Some(0));

    // Valid extra data
    let block_with_valid_extra_data = transform_block(block.clone(), |mut b| {
        b.header.extra_data = BytesMut::zeroed(32).freeze().into();
        b
    });
    assert_matches!(block_with_valid_extra_data.try_into_sealed_block(None), Ok(_));

    // Invalid extra data
    let block_with_invalid_extra_data: Bytes = BytesMut::zeroed(33).freeze();
    let invalid_extra_data_block = transform_block(block.clone(), |mut b| {
        b.header.extra_data = block_with_invalid_extra_data.clone().into();
        b
    });
    assert_matches!(
        invalid_extra_data_block.try_into_sealed_block(None),
        Err(PayloadError::ExtraData(data)) if data == block_with_invalid_extra_data
    );

    // Zero base fee
    let block_with_zero_base_fee = transform_block(block.clone(), |mut b| {
        b.header.base_fee_per_gas = Some(0);
        b
    });
    assert_matches!(
        block_with_zero_base_fee.try_into_sealed_block(None),
        Err(PayloadError::BaseFee(val)) if val == U256::ZERO
    );

    // Invalid encoded transactions
    let mut payload_with_invalid_txs: ExecutionPayloadV1 = block.clone().into();
    payload_with_invalid_txs.transactions.iter_mut().for_each(|tx| {
        *tx = Bytes::new().into();
    });
    let payload_with_invalid_txs = Block::try_from(payload_with_invalid_txs);
    assert_matches!(
        payload_with_invalid_txs,
        Err(PayloadError::Decode(DecodeError::InputTooShort))
    );

    // Non empty ommers
    let block_with_ommers = transform_block(block.clone(), |mut b| {
        b.ommers.push(random_header(&mut rng, 100, None).unseal());
        b
    });
    assert_matches!(
        block_with_ommers.clone().try_into_sealed_block(None),
        Err(PayloadError::BlockHash { consensus, .. })
            if consensus == block_with_ommers.block_hash()
    );

    // None zero difficulty
    let block_with_difficulty = transform_block(block.clone(), |mut b| {
        b.header.difficulty = U256::from(1);
        b
    });
    assert_matches!(
        block_with_difficulty.clone().try_into_sealed_block(None),
        Err(PayloadError::BlockHash { consensus, .. }) if consensus == block_with_difficulty.block_hash()
    );

    // None zero nonce
    let block_with_nonce = transform_block(block.clone(), |mut b| {
        b.header.nonce = 1;
        b
    });
    assert_matches!(
        block_with_nonce.clone().try_into_sealed_block(None),
        Err(PayloadError::BlockHash { consensus, .. }) if consensus == block_with_nonce.block_hash()
    );

    // Valid block
    let valid_block = block;
    assert_matches!(TryInto::<SealedBlock>::try_into(valid_block), Ok(_));
}
