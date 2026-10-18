use crate::QueryService;
use alloy_consensus::{Header, SignableTransaction, TxLegacy, TxType};
use alloy_primitives::{Address, Log, Signature, B256, U256};
use reth_chainspec::ChainSpecBuilder;
use reth_ethereum_primitives::{Block, BlockBody, Receipt};
use reth_primitives_traits::{BlockBody as _, RecoveredBlock};
use reth_provider::{providers::BlockchainProvider, test_utils::MockNodeTypesWithDB};
use reth_pureth_receipt::{test_utils, ObjectKind, ProviderSnapshot, RethRootProvider};
use std::sync::Arc;

pub(crate) type TestQueryService = QueryService<RethRootProvider<MockNodeTypesWithDB>>;

pub(crate) fn provider(count: usize) -> (BlockchainProvider<MockNodeTypesWithDB>, B256) {
    let body = BlockBody {
        transactions: (0..count)
            .map(|nonce| {
                TxLegacy { nonce: nonce as u64, gas_limit: 21_000, ..Default::default() }
                    .into_signed(Signature::new(U256::from(1), U256::from(2), false))
                    .into()
            })
            .collect(),
        ..Default::default()
    };
    let header = Header {
        transactions_root: body.calculate_tx_root(),
        gas_used: count as u64 * 21_000,
        gas_limit: 30_000_000,
        ..Default::default()
    };
    let block = RecoveredBlock::try_new_unhashed(
        Block { header, body },
        vec![Address::repeat_byte(0x66); count],
    )
    .unwrap();
    let receipts = (0..count)
        .map(|index| Receipt {
            tx_type: TxType::Legacy,
            success: true,
            cumulative_gas_used: (index as u64 + 1) * 21_000,
            logs: vec![
                Log::new(
                    Address::repeat_byte(0x11),
                    vec![B256::repeat_byte(0x22)],
                    vec![1, 2, 3].into(),
                )
                .unwrap(),
                Log::new(Address::repeat_byte(0x33), Vec::new(), Default::default()).unwrap(),
            ],
        })
        .collect();
    test_utils::historical_provider_for_block(
        block,
        Some(receipts),
        Arc::new(ChainSpecBuilder::mainnet().byzantium_activated().build()),
    )
}

pub(crate) fn service() -> (TestQueryService, B256) {
    let (provider, hash) = provider(1);
    (QueryService::from_blockchain_provider(provider), hash)
}

pub(crate) fn snapshot(count: usize) -> ProviderSnapshot {
    let (provider, hash) = provider(count);
    RethRootProvider::new(provider).lookup(hash, ObjectKind::Receipts).unwrap()
}
