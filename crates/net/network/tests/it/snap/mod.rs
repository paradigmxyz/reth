//! End-to-end tests for `snap/2` (EIP-8189) request/response serving.
//!
//! These spin up real, connected peers and drive requests through the full path —
//! `SnapClient` request encoding, `RLPx` session transport, `EthRequestHandler`/
//! `StateRangeProviderFactory` serving, and response decoding.

use alloy_consensus::{
    constants::{EMPTY_ROOT_HASH, KECCAK_EMPTY},
    Header,
};
use alloy_eip7928::{
    compute_block_access_list_hash, AccountChanges, BalanceChange, BlockAccessIndex,
};
use alloy_eips::NumHash;
use alloy_primitives::{keccak256, Address, Bytes, B256, U256};
use alloy_rlp::Decodable;
use alloy_trie::{nodes::RlpNode, proof::verify_proof, Nibbles};
use reth_chainspec::Hardforks;
use reth_eth_wire::{
    protocol::Protocol,
    snap::{
        AccountData, AccountRangeMessage, BlockAccessListsMessage, ByteCodesMessage,
        GetAccountRangeMessage, GetBlockAccessListsMessage, GetByteCodesMessage,
        GetStorageRangesMessage, StorageRangesMessage,
    },
    BlockAccessLists, EthVersion,
};
use reth_network::{
    eth_requests::{SlimAccountBody, SOFT_RESPONSE_LIMIT},
    test_utils::{PeerConfig, Testnet, TestnetHandle},
    BlockDownloaderProvider,
};
use reth_network_p2p::snap::client::{SnapClient, SnapResponse};
use reth_primitives_traits::{Account, Block as _, StorageEntry};
use reth_provider::{
    providers::{BlockchainProvider, SNAPSHOT_STATE_RETENTION},
    test_utils::{
        create_test_provider_factory, ExtendedAccount, MockEthProvider, MockNodeTypesWithDB,
    },
    BalProvider, BalStoreHandle, BlockReader, BlockWriter, ChainSpecProvider, HashingWriter,
    HeaderProvider, InMemoryBalStore, ProviderFactory, RawBal, StageCheckpointWriter,
    StateProviderFactory, StateRangeProviderFactory, StateRootProvider, StorageRootProvider,
};
use reth_stages_types::{StageCheckpoint, StageId};
use reth_testing_utils::generators::{self, random_block, BlockParams};
use reth_transaction_pool::test_utils::TestPool;
use reth_trie::{HashedPostState, HashedStorage};
use std::{sync::Arc, time::Duration};

mod protocol;

type SnapTestnetHandle<C> = TestnetHandle<C, TestPool>;

/// Protocols a snap/2-capable peer advertises: `eth/71` plus `snap/2`.
///
/// A session only negotiates the dedicated snap-carrying connection variant for exactly this
/// pair; anything else falls back to a satellite connection that can't serve `GetSnap`.
fn snap_protocols() -> Vec<Protocol> {
    vec![EthVersion::Eth71.into(), Protocol::snap_2()]
}

/// A provider usable by the snap/2 testnet helpers: real block, header, state, bal, and range
/// access.
trait SnapTestProvider:
    BlockReader<
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
    + 'static
{
}

impl<T> SnapTestProvider for T where
    T: BlockReader<
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
        + 'static
{
}

/// Spawns a 2-peer testnet where both peers are snap/2-capable and serve requests against
/// `provider`.
async fn spawn_snap_testnet<C: SnapTestProvider>(provider: C) -> SnapTestnetHandle<C> {
    spawn_snap_testnet_with_protocols(provider, snap_protocols()).await
}

/// Like [`spawn_snap_testnet`], but with a caller-chosen protocol list.
async fn spawn_snap_testnet_with_protocols<C: SnapTestProvider>(
    provider: C,
    protocols: Vec<Protocol>,
) -> SnapTestnetHandle<C> {
    let mut net: Testnet<C, TestPool> = Testnet::default();
    for _ in 0..2 {
        let peer = PeerConfig::with_protocols(provider.clone(), protocols.clone());
        net.add_peer_with_config(peer).await.unwrap();
    }
    net.for_each_mut(|peer| peer.install_request_handler());

    let net = net.spawn();
    net.connect_peers().await;
    net
}

/// A fresh temp-database provider factory with a genesis block and a `StageId::Finish` checkpoint
/// at block 0.
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

/// Commits the current hashed state root to a canonical block and advances the retained-state
/// checkpoint to it.
///
/// Snap requests are keyed by a header-committed root. Writing hashed tables without advancing the
/// chain would make a request for the genesis empty root accidentally serve newer state.
fn persist_fixture_state_root(factory: &ProviderFactory<MockNodeTypesWithDB>) -> B256 {
    let state_root = factory.latest().unwrap().state_root(HashedPostState::default()).unwrap();
    let genesis_hash = factory.sealed_header(0).unwrap().unwrap().hash();
    let mut block = random_block(
        &mut generators::rng(),
        1,
        BlockParams { parent: Some(genesis_hash), tx_count: Some(0), ..Default::default() },
    )
    .unseal();
    block.header.state_root = state_root;
    let block = block.seal_slow();

    let provider_rw = factory.provider_rw().unwrap();
    provider_rw.insert_block(&block.try_recover().unwrap()).unwrap();
    provider_rw.save_stage_checkpoint(StageId::Finish, StageCheckpoint::new(1)).unwrap();
    provider_rw.commit().unwrap();
    state_root
}

/// Verifies one boundary path from the unordered union of trie nodes carried by a snap range
/// proof.
///
/// Snap responses omit each proof node's nibble path and combine both boundary paths into one
/// sorted node list. This finds the subsequence belonging to the requested boundary and passes it
/// to [`verify_proof`], ignoring nodes used only by the other boundary.
fn assert_boundary_proof(root: B256, key: B256, expected_value: Option<Vec<u8>>, proof: &[Bytes]) {
    let root_reference = RlpNode::word_rlp(&root);
    let Some(root_index) = proof
        .iter()
        .position(|node| RlpNode::from_rlp(node).as_slice() == root_reference.as_slice())
    else {
        panic!("proof does not contain the committed root node")
    };

    let key = Nibbles::unpack(key);
    let remaining: Vec<_> = proof
        .iter()
        .enumerate()
        .filter_map(|(index, node)| (index != root_index).then_some(node))
        .collect();
    let mut path = vec![&proof[root_index]];

    fn verifies_subsequence<'a>(
        root: B256,
        key: Nibbles,
        expected_value: &Option<Vec<u8>>,
        remaining: &[&'a Bytes],
        next: usize,
        path: &mut Vec<&'a Bytes>,
    ) -> bool {
        if verify_proof(root, key, expected_value.clone(), path.iter().copied()).is_ok() {
            return true
        }

        for index in next..remaining.len() {
            path.push(remaining[index]);
            if verifies_subsequence(root, key, expected_value, remaining, index + 1, path) {
                return true
            }
            path.pop();
        }
        false
    }

    assert!(
        verifies_subsequence(root, key, &expected_value, &remaining, 0, &mut path),
        "invalid or incomplete proof at boundary {key:?}"
    );
}

/// A valid RLP-encoded EIP-7928 block access list for `address`, with its commitment hash.
fn valid_bal(address: Address) -> (Bytes, B256) {
    let mut change = AccountChanges::new(address);
    change.balance_changes.push(BalanceChange::new(BlockAccessIndex::PRE_EXECUTION, U256::from(1)));
    let bal = vec![change];

    let mut buf = Vec::new();
    alloy_rlp::encode_list(&bal, &mut buf);
    (Bytes::from(buf), compute_block_access_list_hash(&bal))
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

    let state_root = persist_fixture_state_root(&factory);

    let provider = BlockchainProvider::new(factory).unwrap();
    let net = spawn_snap_testnet(provider).await;
    let fetch = net.peers()[0].network().fetch_client().await.unwrap();

    let mut expected: Vec<_> =
        accounts.iter().map(|(address, account)| (keccak256(address), *account)).collect();
    expected.sort_by_key(|(hash, _)| *hash);

    let response = fetch
        .get_account_range(GetAccountRangeMessage {
            request_id: 7,
            root_hash: state_root,
            starting_hash: B256::ZERO,
            // The real highest account hash rather than `B256::repeat_byte(0xff)`: this still
            // returns every account, but the cursor stops via the hash limit instead of
            // exhausting the trie, so a boundary proof is still expected below.
            limit_hash: expected.last().unwrap().0,
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

    assert!(!proof.is_empty());
    assert_boundary_proof(state_root, B256::ZERO, None, &proof);
    let (last_hash, last_account) = expected.last().unwrap();
    assert_boundary_proof(
        state_root,
        *last_hash,
        Some(alloy_rlp::encode(last_account.into_trie_account(EMPTY_ROOT_HASH))),
        &proof,
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn account_range_bounded_by_response_bytes_excludes_trailing_account() {
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

    let state_root = persist_fixture_state_root(&factory);

    let provider = BlockchainProvider::new(factory).unwrap();
    let net = spawn_snap_testnet(provider).await;
    let fetch = net.peers()[0].network().fetch_client().await.unwrap();

    let mut expected: Vec<_> =
        accounts.iter().map(|(address, account)| (keccak256(address), *account)).collect();
    expected.sort_by_key(|(hash, _)| *hash);

    // Each account costs a fixed 160 bytes. A budget below that admits only account A, since the
    // server always returns at least the entry that crosses the limit.
    let response_bytes = 100u64;
    let response = fetch
        .get_account_range(GetAccountRangeMessage {
            request_id: 41,
            root_hash: state_root,
            starting_hash: B256::ZERO,
            limit_hash: B256::repeat_byte(0xff),
            response_bytes,
        })
        .await
        .unwrap()
        .into_data();

    let SnapResponse::AccountRange(AccountRangeMessage { accounts: returned, proof, .. }) =
        response
    else {
        panic!("expected an account range response");
    };
    assert_eq!(returned.len(), 1, "only account A should fit in the byte budget");
    assert_eq!(returned[0].hash, expected[0].0);

    let excluded = expected[1].0;
    assert!(returned.iter().all(|a| a.hash != excluded), "account B must not be returned");

    assert!(!proof.is_empty());
    assert_boundary_proof(state_root, B256::ZERO, None, &proof);
    assert_boundary_proof(
        state_root,
        expected[0].0,
        Some(alloy_rlp::encode(expected[0].1.into_trie_account(EMPTY_ROOT_HASH))),
        &proof,
    );
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

    let state_root = persist_fixture_state_root(&factory);

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
            root_hash: state_root,
            account_hashes: vec![hashed_address],
            starting_hash: origin.into(),
            limit_hash: limit.into(),
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
    assert_boundary_proof(storage_root, origin, Some(alloy_rlp::encode(expected[1].1)), &proof);
    assert_boundary_proof(
        storage_root,
        expected_bounded.last().unwrap().0,
        Some(alloy_rlp::encode(expected_bounded.last().unwrap().1)),
        &proof,
    );
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

    let state_root = persist_fixture_state_root(&factory);

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
            root_hash: state_root,
            account_hashes: vec![hashed_address],
            starting_hash: origin.into(),
            limit_hash: limit.into(),
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
    assert_boundary_proof(storage_root, origin, None, &proof);
    assert_boundary_proof(
        storage_root,
        expected[2].0,
        Some(alloy_rlp::encode(expected[2].1)),
        &proof,
    );
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

    let state_root = persist_fixture_state_root(&factory);

    let provider = BlockchainProvider::new(factory).unwrap();
    let storage_root_a =
        provider.latest().unwrap().storage_root(address_a, HashedStorage::default()).unwrap();
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
            root_hash: state_root,
            account_hashes: vec![hashed_a, hashed_b],
            starting_hash: B256::ZERO.into(),
            limit_hash: B256::repeat_byte(0xff).into(),
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
    assert_boundary_proof(storage_root_b, B256::ZERO, None, &proof);
    let last_b = decoded_b.last().unwrap();
    assert_boundary_proof(storage_root_b, last_b.0, Some(alloy_rlp::encode(last_b.1)), &proof);

    // A non-zero origin on the request only bounds the first account: since it forces a proof for
    // A regardless of budget, B is never reached, proving the origin isn't wrongly applied to B
    // too.
    let origin = expected_a[1].0;
    let response = fetch
        .get_storage_ranges(GetStorageRangesMessage {
            request_id: 22,
            root_hash: state_root,
            account_hashes: vec![hashed_a, hashed_b],
            starting_hash: origin.into(),
            limit_hash: B256::repeat_byte(0xff).into(),
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
    assert_boundary_proof(storage_root_a, origin, Some(alloy_rlp::encode(expected_a[1].1)), &proof);
    let last_a = decoded_a.last().unwrap();
    assert_boundary_proof(storage_root_a, last_a.0, Some(alloy_rlp::encode(last_a.1)), &proof);
}

#[tokio::test(flavor = "multi_thread")]
async fn retained_and_expired_account_range_requests_resolve_without_hanging() {
    reth_tracing::init_test_tracing();

    // Covers request routing only, not changeset reversion.
    let factory = create_test_provider_factory();
    let mut rng = generators::rng();

    // Real trie root of `expired_account` alone, committed to block 0.
    let expired_account =
        (Address::random(), Account { nonce: 3, balance: U256::from(3), bytecode_hash: None });
    {
        let provider_rw = factory.provider_rw().unwrap();
        provider_rw
            .insert_account_for_hashing([(expired_account.0, Some(expired_account.1))])
            .unwrap();
        provider_rw.commit().unwrap();
    }
    let expired_root = factory.latest().unwrap().state_root(HashedPostState::default()).unwrap();

    // Real trie root of `expired_account` + `retained_account` together, committed to block 64.
    let retained_account =
        (Address::random(), Account { nonce: 7, balance: U256::from(7), bytecode_hash: None });
    {
        let provider_rw = factory.provider_rw().unwrap();
        provider_rw
            .insert_account_for_hashing([(retained_account.0, Some(retained_account.1))])
            .unwrap();
        provider_rw.commit().unwrap();
    }
    let retained_root = factory.latest().unwrap().state_root(HashedPostState::default()).unwrap();

    let mut parent = B256::ZERO;
    let provider_rw = factory.provider_rw().unwrap();
    for number in 0..=SNAPSHOT_STATE_RETENTION {
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
    provider_rw
        .save_stage_checkpoint(StageId::Finish, StageCheckpoint::new(SNAPSHOT_STATE_RETENTION))
        .unwrap();
    provider_rw.commit().unwrap();

    let provider = BlockchainProvider::new(factory).unwrap();
    let net = spawn_snap_testnet(provider).await;
    let fetch = net.peers()[0].network().fetch_client().await.unwrap();

    let request = |request_id: u64, root_hash: B256, limit_hash: B256| GetAccountRangeMessage {
        request_id,
        root_hash,
        starting_hash: B256::ZERO,
        limit_hash,
        response_bytes: SOFT_RESPONSE_LIMIT as u64,
    };

    let mut expected: Vec<_> = [expired_account, retained_account]
        .into_iter()
        .map(|(address, account)| (keccak256(address), account))
        .collect();
    expected.sort_by_key(|(hash, _)| *hash);

    // An older root inside the retention window resolves to the account data at that root.
    // Using the real last account hash as the limit forces a hash-limit stop, so a proof is
    // still expected.
    let response = tokio::time::timeout(
        Duration::from_secs(5),
        fetch.get_account_range(request(1, retained_root, expected.last().unwrap().0)),
    )
    .await
    .expect("request should not hang")
    .unwrap()
    .into_data();
    let SnapResponse::AccountRange(AccountRangeMessage { request_id, accounts, proof }) = response
    else {
        panic!("expected an account range response");
    };
    assert_eq!(request_id, 1);
    assert_eq!(accounts.len(), expected.len());
    for (AccountData { hash, .. }, (expected_hash, _)) in accounts.iter().zip(&expected) {
        assert_eq!(hash, expected_hash);
    }
    assert!(!proof.is_empty());
    assert_boundary_proof(retained_root, B256::ZERO, None, &proof);
    let (last_hash, last_account) = expected.last().unwrap();
    assert_boundary_proof(
        retained_root,
        *last_hash,
        Some(alloy_rlp::encode(last_account.into_trie_account(EMPTY_ROOT_HASH))),
        &proof,
    );

    // A root outside the retention window is treated like one that never existed and is
    // unresolvable, so the handler returns an empty response instead of hanging.
    let response = tokio::time::timeout(
        Duration::from_secs(5),
        fetch.get_account_range(request(2, expired_root, B256::repeat_byte(0xff))),
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

#[tokio::test(flavor = "multi_thread")]
async fn block_access_lists_roundtrip_preserves_positions_and_request_id() {
    reth_tracing::init_test_tracing();

    let bal_store = BalStoreHandle::new(InMemoryBalStore::default());
    let mut provider = MockEthProvider::default();
    provider.bal_store = bal_store.clone();
    let provider = Arc::new(provider);

    let hash_a = B256::random();
    let missing_hash = B256::random();
    let hash_b = B256::random();
    let (bal_a, hash_a_commit) = valid_bal(Address::random());
    let (bal_b, hash_b_commit) = valid_bal(Address::random());
    bal_store.insert(NumHash::new(1, hash_a), RawBal::from(bal_a.clone())).unwrap();
    bal_store.insert(NumHash::new(2, hash_b), RawBal::from(bal_b.clone())).unwrap();
    provider.add_header(
        hash_a,
        Header { block_access_list_hash: Some(hash_a_commit), ..Default::default() },
    );
    provider.add_header(
        hash_b,
        Header { block_access_list_hash: Some(hash_b_commit), ..Default::default() },
    );

    let net = spawn_snap_testnet(provider.clone()).await;
    let fetch = net.peers()[0].network().fetch_client().await.unwrap();
    let response = fetch
        .get_block_access_lists(GetBlockAccessListsMessage {
            request_id: 31,
            block_hashes: vec![hash_a, missing_hash, hash_b],
            response_bytes: SOFT_RESPONSE_LIMIT as u64,
        })
        .await
        .unwrap()
        .into_data();

    let SnapResponse::BlockAccessLists(BlockAccessListsMessage { request_id, block_access_lists }) =
        response
    else {
        panic!("expected a block access lists response");
    };
    assert_eq!(request_id, 31);
    assert_eq!(
        block_access_lists,
        BlockAccessLists(vec![Some(bal_a.clone()), None, Some(bal_b.clone())])
    );

    // The served bytes commit to the header's `block_access_list_hash`, per EIP-7928.
    let header_a = provider.header(hash_a).unwrap().unwrap();
    assert_eq!(header_a.block_access_list_hash, Some(keccak256(&bal_a)));
    let header_b = provider.header(hash_b).unwrap().unwrap();
    assert_eq!(header_b.block_access_list_hash, Some(keccak256(&bal_b)));
}

#[tokio::test(flavor = "multi_thread")]
async fn block_access_lists_roundtrip_honors_request_soft_limit() {
    reth_tracing::init_test_tracing();

    let bal_store = BalStoreHandle::new(InMemoryBalStore::default());
    let mut provider = MockEthProvider::default();
    provider.bal_store = bal_store.clone();
    let provider = Arc::new(provider);

    let hash_a = B256::random();
    let missing_hash = B256::random();
    let hash_b = B256::random();
    let (bal_a, _) = valid_bal(Address::random());
    let (bal_b, _) = valid_bal(Address::random());
    bal_store.insert(NumHash::new(1, hash_a), RawBal::from(bal_a.clone())).unwrap();
    bal_store.insert(NumHash::new(2, hash_b), RawBal::from(bal_b)).unwrap();

    let net = spawn_snap_testnet(provider).await;
    let fetch = net.peers()[0].network().fetch_client().await.unwrap();

    // The limit is soft: the missing entry that crosses it is included, then the suffix is
    // truncated. This also checks that an unavailable entry counts as its 0x80 placeholder byte.
    let response = fetch
        .get_block_access_lists(GetBlockAccessListsMessage {
            request_id: 32,
            block_hashes: vec![hash_a, missing_hash, hash_b],
            response_bytes: bal_a.len() as u64,
        })
        .await
        .unwrap()
        .into_data();
    let SnapResponse::BlockAccessLists(BlockAccessListsMessage { block_access_lists, .. }) =
        response
    else {
        panic!("expected a block access lists response");
    };
    assert_eq!(block_access_lists, BlockAccessLists(vec![Some(bal_a.clone()), None]));

    // Even a zero limit returns the first entry that crosses the soft cap rather than dropping the
    // connection or treating zero as an invalid request.
    let response = fetch
        .get_block_access_lists(GetBlockAccessListsMessage {
            request_id: 33,
            block_hashes: vec![hash_a, missing_hash],
            response_bytes: 0,
        })
        .await
        .unwrap()
        .into_data();
    let SnapResponse::BlockAccessLists(BlockAccessListsMessage { block_access_lists, .. }) =
        response
    else {
        panic!("expected a block access lists response");
    };
    assert_eq!(block_access_lists, BlockAccessLists(vec![Some(bal_a)]));

    let response = fetch
        .get_block_access_lists(GetBlockAccessListsMessage {
            request_id: 34,
            block_hashes: Vec::new(),
            response_bytes: SOFT_RESPONSE_LIMIT as u64,
        })
        .await
        .unwrap()
        .into_data();
    let SnapResponse::BlockAccessLists(BlockAccessListsMessage { request_id, block_access_lists }) =
        response
    else {
        panic!("expected a block access lists response");
    };
    assert_eq!(request_id, 34);
    assert_eq!(block_access_lists, BlockAccessLists(Vec::new()));
}

#[tokio::test(flavor = "multi_thread")]
async fn byte_codes_roundtrip_preserves_found_code_order() {
    reth_tracing::init_test_tracing();

    let provider = Arc::new(MockEthProvider::default());
    let code_a = Bytes::from_static(&[0x60, 0x01, 0x60, 0x02, 0x01]);
    let code_b = Bytes::from_static(&[0x5b]);
    let hash_a = keccak256(&code_a);
    let hash_b = keccak256(&code_b);
    let missing_hash = B256::random();

    provider.add_account(
        Address::random(),
        ExtendedAccount::new(0, U256::ZERO).with_bytecode(code_a.clone()),
    );
    provider.add_account(
        Address::random(),
        ExtendedAccount::new(0, U256::ZERO).with_bytecode(code_b.clone()),
    );

    let net = spawn_snap_testnet(provider).await;
    let fetch = net.peers()[0].network().fetch_client().await.unwrap();

    let response = fetch
        .get_byte_codes(GetByteCodesMessage {
            request_id: 13,
            hashes: vec![KECCAK_EMPTY, hash_a, missing_hash, hash_b],
            response_bytes: SOFT_RESPONSE_LIMIT as u64,
        })
        .await
        .unwrap()
        .into_data();

    let SnapResponse::ByteCodes(ByteCodesMessage { request_id, codes }) = response else {
        panic!("expected a byte codes response");
    };
    assert_eq!(request_id, 13);
    // The empty-code hash resolves without a lookup, the missing hash is skipped, and found
    // codes preserve request order rather than storage order.
    assert_eq!(codes, vec![Bytes::new(), code_a, code_b]);
}
