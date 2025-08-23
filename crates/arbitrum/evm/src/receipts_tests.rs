#![cfg(test)]
use alloy_primitives::{b256, Bloom};
use reth_primitives_traits::proofs;
use alloy_consensus::{Header, BlockBody, Block};
use reth_arbitrum_primitives::{ArbTransactionSigned, ArbReceipt};

#[test]
fn receipts_root_matches_header_after_conversion() {
    let receipts: Vec<ArbReceipt> = Vec::new();
    let receipts_root = proofs::calculate_receipt_root(&receipts);

    let header = Header {
        parent_hash: b256!("1111111111111111111111111111111111111111111111111111111111111111"),
        ommers_hash: alloy_consensus::EMPTY_OMMER_ROOT_HASH,
        beneficiary: Default::default(),
        state_root: b256!("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        transactions_root: b256!("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
        receipts_root,
        logs_bloom: Bloom::default(),
        difficulty: 0u64.into(),
        number: 1,
        gas_limit: 20_000_000,
        gas_used: 0,
        timestamp: 1_700_000_000,
        extra_data: Default::default(),
        mix_hash: b256!("2222222222222222222222222222222222222222222222222222222222222222"),
        nonce: Default::default(),
        base_fee_per_gas: Some(1_000_000_000u64),
        withdrawals_root: None,
        blob_gas_used: None,
        excess_blob_gas: None,
        requests_hash: None,
        parent_beacon_block_root: None,
    };
    let block = Block {
        header: header.clone(),
        body: BlockBody { transactions: Vec::<ArbTransactionSigned>::new(), ommers: Vec::new(), withdrawals: None },
    };
    let sealed = block.seal_slow();
    let exec = reth_arbitrum_payload::ArbPayloadTypes::block_to_payload(sealed);
    let v1 = exec.payload.as_v1();
    assert_eq!(v1.receipts_root, receipts_root);
    assert!(v1.withdrawals.is_none());
}
