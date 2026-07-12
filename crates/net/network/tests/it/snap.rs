//! End-to-end tests for `snap/2` (EIP-8189) request/response serving.
//!
//! These spin up real, connected peers and drive requests through the full path —
//! `SnapClient` request encoding, `RLPx` session transport, `EthRequestHandler`/
//! `StateRangeProviderFactory` serving, and response decoding.

use alloy_consensus::constants::EMPTY_ROOT_HASH;
use alloy_primitives::{keccak256, Address, B256, U256};
use alloy_rlp::Decodable;
use reth_chainspec::Hardforks;
use reth_eth_wire::{
    protocol::Protocol,
    snap::{
        AccountData, AccountRangeMessage, GetAccountRangeMessage, GetStorageRangesMessage,
        StorageRangesMessage,
    },
    EthVersion,
};
use reth_network::{
    eth_requests::{SlimAccountBody, SOFT_RESPONSE_LIMIT},
    test_utils::{PeerConfig, Testnet, TestnetHandle},
    BlockDownloaderProvider,
};
use reth_network_p2p::snap::client::{SnapClient, SnapResponse};
use reth_primitives_traits::{Account, Block as _, StorageEntry};
use reth_provider::{
    providers::BlockchainProvider,
    test_utils::{create_test_provider_factory, MockNodeTypesWithDB},
    BalProvider, BlockReader, BlockWriter, ChainSpecProvider, HashingWriter, HeaderProvider,
    ProviderFactory, StageCheckpointWriter, StateProviderFactory, StateRangeProviderFactory,
    StateRootProvider, StorageRootProvider,
};
use reth_stages_types::{StageCheckpoint, StageId};
use reth_testing_utils::generators::{self, random_block, BlockParams};
use reth_transaction_pool::test_utils::TestPool;
use reth_trie::{HashedPostState, HashedStorage};
use std::time::Duration;

type SnapTestnetHandle<C> = TestnetHandle<C, TestPool>;

/// Protocols a snap/2-capable peer advertises: `eth/71` plus `snap/2`.
///
/// A session only negotiates the dedicated snap-carrying connection variant for exactly this
/// pair; anything else falls back to a satellite connection that can't serve `GetSnap`.
fn snap_protocols() -> Vec<Protocol> {
    vec![EthVersion::Eth71.into(), Protocol::snap_2()]
}

/// Spawns a 2-peer testnet where both peers are snap/2-capable and serve requests against
/// `provider`.
async fn spawn_snap_testnet<C>(provider: C) -> SnapTestnetHandle<C>
where
    C: BlockReader<
            Block = reth_ethereum_primitives::Block,
            Receipt = reth_ethereum_primitives::Receipt,
            Header = alloy_consensus::Header,
        > + HeaderProvider
        + BalProvider
        + StateProviderFactory
        + StateRangeProviderFactory
        + ChainSpecProvider<ChainSpec: Hardforks>
        + Clone
        + Unpin
        + 'static,
{
    let mut net: Testnet<C, TestPool> = Testnet::default();
    for _ in 0..2 {
        let peer = PeerConfig::with_protocols(provider.clone(), snap_protocols());
        net.add_peer_with_config(peer).await.unwrap();
    }
    net.for_each_mut(|peer| peer.install_request_handler());

    let net = net.spawn();
    net.connect_peers().await;
    net
}

/// A fresh temp-database provider factory with a genesis block and a `StageId::Finish` checkpoint
/// at block 0, so [`StateRangeProviderFactory::state_range_provider`] can resolve
/// [`EMPTY_ROOT_HASH`] once hashed state is written on top.
fn genesis_provider_factory() -> ProviderFactory<MockNodeTypesWithDB> {
    let factory = create_test_provider_factory();
    let provider_rw = factory.provider_rw().unwrap();
    let mut rng = generators::rng();
    let genesis =
        random_block(&mut rng, 0, BlockParams { tx_count: Some(0), ..Default::default() });
    provider_rw.insert_block(&genesis.try_recover().unwrap()).unwrap();
    provider_rw.save_stage_checkpoint(StageId::Finish, StageCheckpoint::new(0)).unwrap();
    provider_rw.commit().unwrap();
    factory
}

#[tokio::test(flavor = "multi_thread")]
async fn account_range_roundtrip_carries_slim_encoding_and_proof() {
    reth_tracing::init_test_tracing();

    let factory = genesis_provider_factory();
    let accounts: Vec<(Address, Account)> = (0..5u64)
        .map(|nonce| {
            (Address::random(), Account { nonce, balance: U256::from(nonce), bytecode_hash: None })
        })
        .collect();

    let provider_rw = factory.provider_rw().unwrap();
    provider_rw
        .insert_account_for_hashing(
            accounts.iter().map(|(address, account)| (*address, Some(*account))),
        )
        .unwrap();
    provider_rw.commit().unwrap();

    let provider = BlockchainProvider::new(factory).unwrap();
    let state_root = provider.latest().unwrap().state_root(HashedPostState::default()).unwrap();

    let net = spawn_snap_testnet(provider).await;
    let fetch = net.peers()[0].network().fetch_client().await.unwrap();

    let mut expected: Vec<_> =
        accounts.iter().map(|(address, account)| (keccak256(address), *account)).collect();
    expected.sort_by_key(|(hash, _)| *hash);

    let response = fetch
        .get_account_range(GetAccountRangeMessage {
            request_id: 7,
            root_hash: EMPTY_ROOT_HASH,
            starting_hash: B256::ZERO,
            limit_hash: B256::repeat_byte(0xff),
            response_bytes: SOFT_RESPONSE_LIMIT as u64,
        })
        .await
        .unwrap()
        .into_data();

    let SnapResponse::AccountRange(AccountRangeMessage { request_id, accounts: returned, proof }) =
        response
    else {
        panic!("expected an account range response");
    };
    assert_eq!(request_id, 7);
    assert_eq!(returned.len(), expected.len());
    for (AccountData { hash, body }, (expected_hash, expected_account)) in
        returned.iter().zip(&expected)
    {
        assert_eq!(hash, expected_hash);
        let decoded = SlimAccountBody::decode(&mut &body[..]).unwrap();
        assert_eq!(decoded.nonce, expected_account.nonce);
        assert_eq!(decoded.balance, expected_account.balance);
        // Freshly generated EOAs have no storage/code, so they get the slim (elided) encoding.
        assert!(decoded.storage_root.is_empty());
        assert!(decoded.code_hash.is_empty());
    }

    // A boundary proof's first node is always the trie root, so this proves the wire response
    // carries a real proof against the requested state root, not an empty placeholder.
    assert!(!proof.is_empty());
    assert_eq!(keccak256(&proof[0]), state_root);
}

#[tokio::test(flavor = "multi_thread")]
async fn storage_range_roundtrip_carries_rlp_values_and_proof() {
    reth_tracing::init_test_tracing();

    let factory = genesis_provider_factory();
    let address = Address::random();
    let account = Account { nonce: 1, balance: U256::from(1), bytecode_hash: None };
    let slots: Vec<StorageEntry> = (0..6u8)
        .map(|i| StorageEntry { key: B256::with_last_byte(i), value: U256::from(i as u64 + 1) })
        .collect();

    let provider_rw = factory.provider_rw().unwrap();
    provider_rw.insert_account_for_hashing([(address, Some(account))]).unwrap();
    provider_rw.insert_storage_for_hashing([(address, slots.clone())]).unwrap();
    provider_rw.commit().unwrap();

    let provider = BlockchainProvider::new(factory).unwrap();
    let storage_root =
        provider.latest().unwrap().storage_root(address, HashedStorage::default()).unwrap();

    let net = spawn_snap_testnet(provider).await;
    let fetch = net.peers()[0].network().fetch_client().await.unwrap();

    let hashed_address = keccak256(address);
    let mut expected: Vec<_> =
        slots.iter().map(|entry| (keccak256(entry.key), entry.value)).collect();
    expected.sort_by_key(|(hash, _)| *hash);

    // A bounded, non-zero origin needs a boundary proof: request the middle third of the range.
    let origin = expected[1].0;
    let limit = expected[3].0;
    let response = fetch
        .get_storage_ranges(GetStorageRangesMessage {
            request_id: 9,
            root_hash: EMPTY_ROOT_HASH,
            account_hashes: vec![hashed_address],
            starting_hash: origin,
            limit_hash: limit,
            response_bytes: SOFT_RESPONSE_LIMIT as u64,
        })
        .await
        .unwrap()
        .into_data();

    let SnapResponse::StorageRanges(StorageRangesMessage { request_id, slots: returned, proof }) =
        response
    else {
        panic!("expected a storage ranges response");
    };
    assert_eq!(request_id, 9);
    assert_eq!(returned.len(), 1);

    let decoded: Vec<_> = returned[0]
        .iter()
        .map(|slot| (slot.hash, U256::decode(&mut &slot.data[..]).unwrap()))
        .collect();
    let expected_bounded: Vec<_> =
        expected.iter().filter(|(hash, _)| *hash >= origin && *hash <= limit).copied().collect();
    assert_eq!(decoded, expected_bounded);

    assert!(!proof.is_empty());
    assert_eq!(keccak256(&proof[0]), storage_root);
}

#[tokio::test(flavor = "multi_thread")]
async fn storage_range_empty_window_returns_boundary_slot() {
    reth_tracing::init_test_tracing();

    let factory = genesis_provider_factory();
    let address = Address::random();
    let account = Account { nonce: 1, balance: U256::from(1), bytecode_hash: None };
    let slots: Vec<StorageEntry> = (0..4u8)
        .map(|i| StorageEntry { key: B256::with_last_byte(i), value: U256::from(i as u64 + 1) })
        .collect();

    let provider_rw = factory.provider_rw().unwrap();
    provider_rw.insert_account_for_hashing([(address, Some(account))]).unwrap();
    provider_rw.insert_storage_for_hashing([(address, slots.clone())]).unwrap();
    provider_rw.commit().unwrap();

    let provider = BlockchainProvider::new(factory).unwrap();
    let storage_root =
        provider.latest().unwrap().storage_root(address, HashedStorage::default()).unwrap();

    let net = spawn_snap_testnet(provider).await;
    let fetch = net.peers()[0].network().fetch_client().await.unwrap();

    let hashed_address = keccak256(address);
    let mut expected: Vec<_> =
        slots.iter().map(|entry| (keccak256(entry.key), entry.value)).collect();
    expected.sort_by_key(|(hash, _)| *hash);

    // A window strictly between the 2nd- and 3rd-lowest hashes contains no slots; the response
    // must still return the first slot past it, proven with a boundary proof (non-zero origin
    // always requires one).
    let origin = B256::from(U256::from_be_bytes(expected[1].0 .0) + U256::from(1));
    let limit = origin;
    let response = fetch
        .get_storage_ranges(GetStorageRangesMessage {
            request_id: 11,
            root_hash: EMPTY_ROOT_HASH,
            account_hashes: vec![hashed_address],
            starting_hash: origin,
            limit_hash: limit,
            response_bytes: SOFT_RESPONSE_LIMIT as u64,
        })
        .await
        .unwrap()
        .into_data();

    let SnapResponse::StorageRanges(StorageRangesMessage { slots: returned, proof, .. }) = response
    else {
        panic!("expected a storage ranges response");
    };
    assert_eq!(returned.len(), 1);
    let decoded: Vec<_> = returned[0]
        .iter()
        .map(|slot| (slot.hash, U256::decode(&mut &slot.data[..]).unwrap()))
        .collect();
    assert_eq!(decoded, vec![expected[2]]);

    assert!(!proof.is_empty());
    assert_eq!(keccak256(&proof[0]), storage_root);
}

#[tokio::test(flavor = "multi_thread")]
async fn storage_ranges_multi_account_bounds_only_first_account() {
    reth_tracing::init_test_tracing();

    let factory = genesis_provider_factory();
    let (address_a, account_a) =
        (Address::random(), Account { nonce: 1, balance: U256::from(1), bytecode_hash: None });
    let (address_b, account_b) =
        (Address::random(), Account { nonce: 2, balance: U256::from(2), bytecode_hash: None });
    // 2 slots for A (fits fully in the byte budget below), 5 for B (doesn't).
    let slots_a: Vec<StorageEntry> = (0..2u8)
        .map(|i| StorageEntry { key: B256::with_last_byte(i), value: U256::from(i as u64 + 1) })
        .collect();
    let slots_b: Vec<StorageEntry> = (0..5u8)
        .map(|i| StorageEntry { key: B256::with_last_byte(i), value: U256::from(i as u64 + 10) })
        .collect();

    let provider_rw = factory.provider_rw().unwrap();
    provider_rw
        .insert_account_for_hashing([(address_a, Some(account_a)), (address_b, Some(account_b))])
        .unwrap();
    provider_rw
        .insert_storage_for_hashing([(address_a, slots_a.clone()), (address_b, slots_b.clone())])
        .unwrap();
    provider_rw.commit().unwrap();

    let provider = BlockchainProvider::new(factory).unwrap();
    let storage_root_b =
        provider.latest().unwrap().storage_root(address_b, HashedStorage::default()).unwrap();

    let net = spawn_snap_testnet(provider).await;
    let fetch = net.peers()[0].network().fetch_client().await.unwrap();

    let hashed_a = keccak256(address_a);
    let hashed_b = keccak256(address_b);
    let mut expected_a: Vec<_> = slots_a.iter().map(|e| (keccak256(e.key), e.value)).collect();
    expected_a.sort_by_key(|(hash, _)| *hash);
    let mut expected_b: Vec<_> = slots_b.iter().map(|e| (keccak256(e.key), e.value)).collect();
    expected_b.sort_by_key(|(hash, _)| *hash);

    // A's 2 slots (128 "bytes") fit fully; the remaining 96-byte budget lets B's range start but
    // not finish, so A comes back complete and unproven while B comes back partial and proven.
    let response_bytes = 224u64;
    let response = fetch
        .get_storage_ranges(GetStorageRangesMessage {
            request_id: 21,
            root_hash: EMPTY_ROOT_HASH,
            account_hashes: vec![hashed_a, hashed_b],
            starting_hash: B256::ZERO,
            limit_hash: B256::repeat_byte(0xff),
            response_bytes,
        })
        .await
        .unwrap()
        .into_data();
    let SnapResponse::StorageRanges(StorageRangesMessage { slots: returned, proof, .. }) = response
    else {
        panic!("expected a storage ranges response");
    };
    assert_eq!(returned.len(), 2, "both accounts should appear");
    let decoded_a: Vec<_> = returned[0]
        .iter()
        .map(|slot| (slot.hash, U256::decode(&mut &slot.data[..]).unwrap()))
        .collect();
    assert_eq!(decoded_a, expected_a, "the earlier account's range should be complete");
    let decoded_b: Vec<_> = returned[1]
        .iter()
        .map(|slot| (slot.hash, U256::decode(&mut &slot.data[..]).unwrap()))
        .collect();
    assert!(decoded_b.len() < expected_b.len(), "the final account's range should be truncated");
    assert_eq!(decoded_b, expected_b[..decoded_b.len()]);
    assert!(!proof.is_empty());
    assert_eq!(keccak256(&proof[0]), storage_root_b, "the proof should cover the final account");

    // A non-zero origin on the request only bounds the first account: since it forces a proof for
    // A regardless of budget, B is never reached, proving the origin isn't (mis)applied to B too.
    let origin = expected_a[1].0;
    let response = fetch
        .get_storage_ranges(GetStorageRangesMessage {
            request_id: 22,
            root_hash: EMPTY_ROOT_HASH,
            account_hashes: vec![hashed_a, hashed_b],
            starting_hash: origin,
            limit_hash: B256::repeat_byte(0xff),
            response_bytes: SOFT_RESPONSE_LIMIT as u64,
        })
        .await
        .unwrap()
        .into_data();
    let SnapResponse::StorageRanges(StorageRangesMessage { slots: returned, proof, .. }) = response
    else {
        panic!("expected a storage ranges response");
    };
    assert_eq!(returned.len(), 1, "only the bounded first account should appear");
    let decoded_a: Vec<_> = returned[0]
        .iter()
        .map(|slot| (slot.hash, U256::decode(&mut &slot.data[..]).unwrap()))
        .collect();
    assert_eq!(decoded_a, expected_a[1..]);
    assert!(!proof.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn retained_and_expired_state_roots_respond_without_hanging() {
    reth_tracing::init_test_tracing();

    // Mirrors `SNAPSHOT_STATE_RETENTION` in `blockchain_provider.rs`: the window of the most
    // recent blocks whose state roots are still resolvable.
    const RETENTION: u64 = 128;

    let factory = create_test_provider_factory();
    let mut rng = generators::rng();
    let expired_root = B256::repeat_byte(0x11);
    let retained_root = B256::repeat_byte(0x22);
    let mut parent = B256::ZERO;

    let provider_rw = factory.provider_rw().unwrap();
    for number in 0..=RETENTION {
        let mut block = random_block(
            &mut rng,
            number,
            BlockParams { parent: Some(parent), tx_count: Some(0), ..Default::default() },
        )
        .unseal();
        block.header.state_root = match number {
            0 => expired_root,
            64 => retained_root,
            _ => EMPTY_ROOT_HASH,
        };
        let block = block.seal_slow();
        parent = block.hash();
        provider_rw.insert_block(&block.try_recover().unwrap()).unwrap();
    }
    provider_rw.save_stage_checkpoint(StageId::Finish, StageCheckpoint::new(RETENTION)).unwrap();

    let account =
        (Address::random(), Account { nonce: 7, balance: U256::from(7), bytecode_hash: None });
    provider_rw.insert_account_for_hashing([(account.0, Some(account.1))]).unwrap();
    provider_rw.commit().unwrap();

    let provider = BlockchainProvider::new(factory).unwrap();
    let net = spawn_snap_testnet(provider).await;
    let fetch = net.peers()[0].network().fetch_client().await.unwrap();

    let request = |request_id: u64, root_hash: B256| GetAccountRangeMessage {
        request_id,
        root_hash,
        starting_hash: B256::ZERO,
        limit_hash: B256::repeat_byte(0xff),
        response_bytes: SOFT_RESPONSE_LIMIT as u64,
    };

    // An older root still inside the retention window resolves to the real account data as of
    // that root, even though the chain has advanced well past it.
    let response = tokio::time::timeout(
        Duration::from_secs(5),
        fetch.get_account_range(request(1, retained_root)),
    )
    .await
    .expect("request should not hang")
    .unwrap()
    .into_data();
    let SnapResponse::AccountRange(AccountRangeMessage { request_id, accounts, .. }) = response
    else {
        panic!("expected an account range response");
    };
    assert_eq!(request_id, 1);
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].hash, keccak256(account.0));

    // A root outside the retention window (same code path as a root that never existed) is
    // unresolvable, so the handler returns the typed "unavailable" empty response instead of
    // hanging or falling back to newer state.
    let response = tokio::time::timeout(
        Duration::from_secs(5),
        fetch.get_account_range(request(2, expired_root)),
    )
    .await
    .expect("request should not hang")
    .unwrap()
    .into_data();
    let SnapResponse::AccountRange(AccountRangeMessage { request_id, accounts, proof }) = response
    else {
        panic!("expected an account range response");
    };
    assert_eq!(request_id, 2);
    assert!(accounts.is_empty());
    assert!(proof.is_empty());
}
