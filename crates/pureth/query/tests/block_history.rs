#![allow(missing_docs, rustdoc::missing_crate_level_docs)]
#![cfg(not(target_arch = "wasm32"))]

use alloy_consensus::Header;
use alloy_eips::eip2935::{HISTORY_SERVE_WINDOW, HISTORY_STORAGE_ADDRESS, HISTORY_STORAGE_CODE};
use alloy_evm::block::system_calls::SystemCaller;
use alloy_primitives::{Address, Bytes, TxKind, B256, U256};
use reth_chainspec::ChainSpecBuilder;
use reth_evm::{ConfigureEvm, Evm};
use reth_evm_ethereum::EthEvmConfig;
use reth_primitives_traits::Account;
use reth_provider::EvmStateProviderAdapter;
use reth_revm::{
    cached::CachedReads, database::StateProviderDatabase, test_utils::StateProviderTest,
};
use revm::{context::TxEnv, database::CacheDB, Database};
use std::sync::Arc;

fn state() -> (StateProviderTest, Address, Address) {
    let caller = Address::repeat_byte(0x11);
    let contract = Address::repeat_byte(0x22);
    let mut source = StateProviderTest::default();
    source.insert_account(
        caller,
        Account { balance: U256::from(10u64.pow(18)), ..Default::default() },
        None,
        Default::default(),
    );
    source.insert_account(
        contract,
        Account::default(),
        Some(Bytes::from_static(&[
            0x60, 0x00, 0x35, 0x40, 0x60, 0x00, 0x52, 0x60, 0x20, 0x60, 0x00, 0xf3,
        ])),
        Default::default(),
    );
    (source, caller, contract)
}

fn header(number: u64) -> Header {
    Header {
        number,
        timestamp: 1,
        gas_limit: 30_000_000,
        base_fee_per_gas: Some(0),
        excess_blob_gas: Some(0),
        ..Default::default()
    }
}

fn transaction(caller: Address, contract: Address, number: U256) -> TxEnv {
    TxEnv {
        caller,
        kind: TxKind::Call(contract),
        gas_limit: 100_000,
        gas_price: 0,
        data: Bytes::copy_from_slice(&number.to_be_bytes::<32>()),
        chain_id: Some(1),
        ..Default::default()
    }
}

#[test]
fn blockhash_contract_preserves_ancestor_eligibility_through_reth_cached_reads() {
    let (mut source, caller, contract) = state();
    let recent = B256::repeat_byte(0x44);
    let oldest = B256::repeat_byte(0x55);
    for (number, hash) in [(999, recent), (744, oldest), (743, B256::repeat_byte(0x66))] {
        source.insert_block_hash(number, hash);
    }
    let spec = Arc::new(ChainSpecBuilder::mainnet().prague_activated().build());
    let config = EthEvmConfig::ethereum(spec);
    let mut cache = CachedReads::default();
    {
        let database = StateProviderDatabase::new(EvmStateProviderAdapter(&source));
        let mut evm = config.evm_for_block(cache.as_db_mut(database), &header(1000)).unwrap();
        for (number, expected) in [
            (U256::from(999), recent),
            (U256::from(999), recent),
            (U256::from(744), oldest),
            (U256::from(743), B256::ZERO),
            (U256::from(1000), B256::ZERO),
            (U256::from(1001), B256::ZERO),
            (U256::MAX, B256::ZERO),
            (U256::from(998), B256::ZERO),
        ] {
            let result = evm.transact(transaction(caller, contract, number)).unwrap().result;
            assert!(result.is_success());
            assert_eq!(result.output().unwrap().as_ref(), expected.as_slice());
        }
    }
    assert_eq!(cache.block_hashes.len(), 3);
    assert_eq!(cache.block_hashes.get(&999), Some(&recent));
    assert_eq!(cache.block_hashes.get(&744), Some(&oldest));
    assert_eq!(cache.block_hashes.get(&998), Some(&B256::ZERO));
}

#[test]
fn parent_history_system_call_stores_the_supplied_identity_only_when_active() {
    for (active, number) in [(false, 1000), (true, 0), (true, 1000)] {
        let (mut source, caller, _) = state();
        source.insert_account(
            HISTORY_STORAGE_ADDRESS,
            Account { nonce: 1, ..Default::default() },
            Some(HISTORY_STORAGE_CODE.clone()),
            Default::default(),
        );
        let builder = ChainSpecBuilder::mainnet();
        let spec = Arc::new(if active {
            builder.prague_activated().build()
        } else {
            builder.cancun_activated().build()
        });
        let config = EthEvmConfig::ethereum(spec.clone());
        let database = StateProviderDatabase::new(EvmStateProviderAdapter(&source));
        let mut evm = config.evm_for_block(CacheDB::new(database), &header(number)).unwrap();
        let parent = B256::repeat_byte(0x77);
        SystemCaller::new(spec).apply_blockhashes_contract_call(parent, &mut evm).unwrap();
        let slot = U256::from(number.saturating_sub(1)) % U256::from(HISTORY_SERVE_WINDOW);
        let stored = evm.db_mut().storage(HISTORY_STORAGE_ADDRESS, slot).unwrap();
        let expected =
            if active && number != 0 { U256::from_be_bytes(parent.0) } else { U256::ZERO };
        assert_eq!(stored, expected);
        if active && number != 0 {
            let result = evm
                .transact(transaction(caller, HISTORY_STORAGE_ADDRESS, U256::from(number - 1)))
                .unwrap()
                .result;
            assert!(result.is_success());
            assert_eq!(result.output().unwrap().as_ref(), parent.as_slice());
        }
    }
}
