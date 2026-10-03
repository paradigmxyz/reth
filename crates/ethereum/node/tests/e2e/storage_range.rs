//! E2E tests for `debug_storageRangeAt`.

use alloy_eips::BlockId;
use alloy_genesis::GenesisAccount;
use alloy_network::TransactionBuilder;
use alloy_primitives::{bytes, keccak256, Address, Bytes, TxKind, B256, U256};
use alloy_provider::Provider;
use alloy_rpc_types_eth::TransactionRequest;
use eyre::eyre;
use reth_chainspec::EthereumHardfork;
use reth_e2e_test_utils::{
    receipt::PendingTransactionExt, test_chain_spec_builder, test_genesis, E2ETestSetupExt,
};
use reth_node_ethereum::EthereumNode;
use reth_rpc_api::HashedStorageRangeResult;
use std::{collections::BTreeMap, sync::Arc};

const MAX_FEE_PER_GAS: u128 = 20_000_000_000;
const MAX_PRIORITY_FEE_PER_GAS: u128 = 1_000_000_000;

/// Storage slots the storage-writing contract is allocated with in genesis.
const SEEDED_SLOTS: u64 = 4;

/// Runtime code that executes `sstore(calldataload(0), calldataload(0x20))`.
const fn storage_writer_code() -> Bytes {
    bytes!("6020356000355500")
}

/// Returns the genesis storage of the storage-writing contract: slot `i` holds `i + 1`.
fn seeded_storage() -> BTreeMap<B256, B256> {
    (0..SEEDED_SLOTS)
        .map(|slot| (B256::from(U256::from(slot)), B256::from(U256::from(slot + 1))))
        .collect()
}

/// Returns a transaction that makes the storage-writing contract store `value` at `slot`.
fn write_tx(
    from: Address,
    contract: Address,
    nonce: u64,
    slot: B256,
    value: B256,
) -> TransactionRequest {
    let mut calldata = slot.to_vec();
    calldata.extend_from_slice(value.as_slice());
    TransactionRequest::default()
        .with_from(from)
        .with_to(contract)
        .with_nonce(nonce)
        .with_gas_limit(100_000)
        .with_max_fee_per_gas(MAX_FEE_PER_GAS)
        .with_max_priority_fee_per_gas(MAX_PRIORITY_FEE_PER_GAS)
        .with_input(calldata)
}

/// Calls `debug_storageRangeAt`, passing the block as a plain hash like geth clients do.
async fn storage_range_at<P: Provider>(
    provider: &P,
    block_hash: B256,
    tx_index: usize,
    address: Address,
    key_start: Bytes,
    max_result: u64,
) -> eyre::Result<HashedStorageRangeResult> {
    Ok(provider
        .client()
        .request("debug_storageRangeAt", (block_hash, tx_index, address, key_start, max_result))
        .await?)
}

/// Returns the hash and transaction count of the latest block.
async fn latest_block<P: Provider>(provider: &P) -> eyre::Result<(B256, usize)> {
    let block =
        provider.get_block(BlockId::latest()).await?.ok_or_else(|| eyre!("missing block"))?;
    Ok((block.header.hash, block.transactions.len()))
}

/// Returns the values of a storage range, keyed by hashed slot.
fn values(range: &HashedStorageRangeResult) -> BTreeMap<B256, B256> {
    range.storage.iter().map(|(hashed_slot, entry)| (*hashed_slot, entry.value)).collect()
}

/// Exercises `debug_storageRangeAt` across the transactions of a block that writes storage, and
/// against an earlier block once the chain has moved on.
#[tokio::test]
async fn storage_range_at_replays_block_transactions() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let contract = Address::repeat_byte(0xc0);
    let mut genesis = test_genesis();
    genesis.alloc.insert(
        contract,
        GenesisAccount::default()
            .with_code(Some(storage_writer_code()))
            .with_storage(Some(seeded_storage())),
    );
    let chain_spec =
        Arc::new(test_chain_spec_builder().genesis(genesis).cancun_activated().build());

    let (mut node, wallet) = EthereumNode::test_setup(1, chain_spec).build_single().await?;
    let signer = wallet.inner.clone();
    let provider = node.rpc_provider_with_wallet(signer.clone());

    let slot_a = B256::from(U256::from(0xaa));
    let value_a = B256::from(U256::from(0x11));
    let slot_b = B256::from(U256::from(0xbb));
    let value_b = B256::from(U256::from(0x22));

    // Three transactions in one block: two write new slots, the last one clears a seeded slot.
    let writes = [(slot_a, value_a), (slot_b, value_b), (B256::ZERO, B256::ZERO)];
    let mut pending = Vec::with_capacity(writes.len());
    for (nonce, (slot, value)) in writes.iter().enumerate() {
        let tx = write_tx(signer.address(), contract, nonce as u64, *slot, *value);
        pending.push(provider.send_transaction(tx).await?);
    }
    node.advance_block().await?;
    for tx in pending {
        tx.successful_receipt().await?;
    }
    let (block_hash, transaction_count) = latest_block(&provider).await?;
    assert_eq!(transaction_count, writes.len());

    let seeded = seeded_storage()
        .into_iter()
        .map(|(slot, value)| (keccak256(slot), value))
        .collect::<BTreeMap<_, _>>();

    // Before the first transaction only the genesis storage is visible, and without preimages.
    let range = storage_range_at(&provider, block_hash, 0, contract, Bytes::new(), 100).await?;
    assert_eq!(range.next_key, None);
    assert_eq!(values(&range), seeded);
    assert!(range.storage.values().all(|entry| entry.key.is_none()));

    // Each replayed transaction reveals the slot it wrote, with its preimage.
    let mut expected = seeded.clone();
    expected.insert(keccak256(slot_a), value_a);
    let range = storage_range_at(&provider, block_hash, 1, contract, Bytes::new(), 100).await?;
    assert_eq!(values(&range), expected);
    assert_eq!(range.storage[&keccak256(slot_a)].key, Some(slot_a));

    expected.insert(keccak256(slot_b), value_b);
    let range = storage_range_at(&provider, block_hash, 2, contract, Bytes::new(), 100).await?;
    assert_eq!(values(&range), expected);
    assert_eq!(range.storage[&keccak256(slot_b)].key, Some(slot_b));

    // The transaction count addresses the state after the last transaction, which cleared a slot.
    expected.remove(&keccak256(B256::ZERO));
    let full = storage_range_at(&provider, block_hash, 3, contract, Bytes::new(), 100).await?;
    assert_eq!(values(&full), expected);
    assert_eq!(full.next_key, None);

    // An index past the transaction count is rejected.
    let err = provider
        .client()
        .request::<_, HashedStorageRangeResult>(
            "debug_storageRangeAt",
            (block_hash, 4, contract, Bytes::new(), 100),
        )
        .await
        .unwrap_err();
    assert_eq!(err.as_error_resp().map(|payload| payload.code), Some(-32602));

    // The genesis block has no parent state to replay on.
    let genesis =
        provider.get_block(BlockId::number(0)).await?.ok_or_else(|| eyre!("missing genesis"))?;
    let err = provider
        .client()
        .request::<_, HashedStorageRangeResult>(
            "debug_storageRangeAt",
            (genesis.header.hash, 0, contract, Bytes::new(), 100),
        )
        .await
        .unwrap_err();
    let payload = err.as_error_resp().ok_or_else(|| eyre!("expected an error response"))?;
    assert_eq!((payload.code, payload.message.as_ref()), (-32000, "genesis is not traceable"));

    // Paging walks the same entries.
    let mut paged = BTreeMap::new();
    let mut start = Bytes::new();
    loop {
        let page = storage_range_at(&provider, block_hash, 3, contract, start, 2).await?;
        assert!(page.storage.len() <= 2);
        paged.extend(page.storage);

        let Some(next_key) = page.next_key else { break };
        start = Bytes::from(next_key.0);
    }
    assert_eq!(paged, full.storage);

    // A later block overwrites a slot.
    let value_a_updated = B256::from(U256::from(0x33));
    let tx = write_tx(signer.address(), contract, writes.len() as u64, slot_a, value_a_updated);
    let pending = provider.send_transaction(tx).await?;
    node.advance_block().await?;
    pending.successful_receipt().await?;
    let (next_block_hash, _) = latest_block(&provider).await?;

    // The earlier block still reports its own state rather than the latest one.
    let range = storage_range_at(&provider, block_hash, 3, contract, Bytes::new(), 100).await?;
    assert_eq!(range, full);

    // The later block starts from that state. Nothing was replayed, so no preimage is known.
    let range =
        storage_range_at(&provider, next_block_hash, 0, contract, Bytes::new(), 100).await?;
    assert_eq!(values(&range), expected);
    assert!(range.storage.values().all(|entry| entry.key.is_none()));

    expected.insert(keccak256(slot_a), value_a_updated);
    let range =
        storage_range_at(&provider, next_block_hash, 1, contract, Bytes::new(), 100).await?;
    assert_eq!(values(&range), expected);
    assert_eq!(range.storage[&keccak256(slot_a)].key, Some(slot_a));

    // An account without storage yields an empty page.
    let unknown = Address::repeat_byte(0xee);
    let range = storage_range_at(&provider, next_block_hash, 1, unknown, Bytes::new(), 100).await?;
    assert_eq!(range, HashedStorageRangeResult::default());

    Ok(())
}

/// Returns init code for a contract that stores `0x42` at slot 0 when deployed and
/// selfdestructs on any call.
fn selfdestruct_contract_init_code() -> Bytes {
    // PUSH20 <beneficiary>, SELFDESTRUCT.
    let runtime = bytes!("73dead000000000000000000000000000000000001ff");
    let runtime_len = runtime.len() as u8;
    // Length of the init code that precedes the runtime code.
    let init_len = 17u8;

    let mut init = Vec::new();
    // PUSH1 0x42, PUSH1 0x00, SSTORE.
    init.extend_from_slice(&[0x60, 0x42, 0x60, 0x00, 0x55]);
    // CODECOPY the runtime code to memory.
    init.extend_from_slice(&[0x60, runtime_len, 0x60, init_len, 0x60, 0x00, 0x39]);
    // RETURN it.
    init.extend_from_slice(&[0x60, runtime_len, 0x60, 0x00, 0xf3]);
    init.extend_from_slice(&runtime);

    Bytes::from(init)
}

/// Before Cancun, `SELFDESTRUCT` wipes an account's storage. Once a transaction of the block
/// destroyed the account, its storage in the parent state must not be reported anymore.
#[tokio::test]
async fn storage_range_at_hides_storage_of_destroyed_account() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let (mut node, wallet) =
        EthereumNode::test_setup_for(EthereumHardfork::Shanghai).build_single().await?;
    let signer = wallet.inner.clone();
    let provider = node.rpc_provider_with_wallet(signer.clone());

    let deploy = TransactionRequest::default()
        .with_from(signer.address())
        .with_nonce(0)
        .with_gas_limit(500_000)
        .with_max_fee_per_gas(MAX_FEE_PER_GAS)
        .with_max_priority_fee_per_gas(MAX_PRIORITY_FEE_PER_GAS)
        .with_input(selfdestruct_contract_init_code())
        .with_kind(TxKind::Create);
    let pending = provider.send_transaction(deploy).await?;
    node.advance_block().await?;
    let receipt = pending.successful_receipt().await?;
    let contract = receipt.contract_address.ok_or_else(|| eyre!("missing contract address"))?;

    let destroy = TransactionRequest::default()
        .with_from(signer.address())
        .with_to(contract)
        .with_nonce(1)
        .with_gas_limit(100_000)
        .with_max_fee_per_gas(MAX_FEE_PER_GAS)
        .with_max_priority_fee_per_gas(MAX_PRIORITY_FEE_PER_GAS);
    let pending = provider.send_transaction(destroy).await?;
    node.advance_block().await?;
    pending.successful_receipt().await?;
    let (block_hash, transaction_count) = latest_block(&provider).await?;
    assert_eq!(transaction_count, 1);

    // The destroying transaction still runs on the deployed storage.
    let range = storage_range_at(&provider, block_hash, 0, contract, Bytes::new(), 100).await?;
    assert_eq!(range.next_key, None);
    assert_eq!(
        values(&range),
        BTreeMap::from([(keccak256(B256::ZERO), B256::from(U256::from(0x42)))])
    );

    // After it, the account and its storage are gone.
    let range = storage_range_at(&provider, block_hash, 1, contract, Bytes::new(), 100).await?;
    assert_eq!(range, HashedStorageRangeResult::default());

    Ok(())
}
