//! Tests for the key encodings of the trie tables.

use alloy_primitives::{bytes, Address, Bytes, B256, U256};
use reth_chainspec::EthereumHardfork;
use reth_db::{
    cursor::DbCursorRO,
    table::Table,
    tables::{MemcmpAccountsTrie, MemcmpStoragesTrie, PackedAccountsTrie, PackedStoragesTrie},
    transaction::DbTx,
    version::{get_db_version, MEMCMP_TRIE_KEYS_DB_VERSION},
};
use reth_e2e_test_utils::{trie::assert_trie_consistency, E2ETestSetupExt, NodeHelperType};
use reth_node_core::args::StorageArgs;
use reth_node_ethereum::EthereumNode;
use reth_provider::{DatabaseProviderFactory, MetadataProvider, StoragePath, StorageSettings};

// Init code of a contract that stores calldata word 1 at the key in word 0: PUSH1 32 CALLDATALOAD
// PUSH0 CALLDATALOAD SSTORE STOP.
const STORE_INIT_CODE: Bytes = bytes!("666020355f3555005f5260076019f3");

/// A node with memcmp trie keys validates the blocks of a node with packed trie keys and stores
/// the same trie nodes, also after it restarts.
#[tokio::test]
async fn memcmp_trie_keys_store_same_trie_as_packed() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    // Both nodes persist every block, so state roots of later blocks are computed from the trie
    // tables.
    let (mut packed, wallet) = EthereumNode::test_setup_for(EthereumHardfork::Prague)
        .with_tree_config_modifier(|config| {
            config.with_persistence_threshold(0).with_memory_block_buffer_target(0)
        })
        .build_single()
        .await?;
    let (memcmp, _) = EthereumNode::test_setup_for(EthereumHardfork::Prague)
        .with_restartable_nodes()
        .with_node_config_modifier(|config| {
            let storage = StorageArgs { memcmp_trie_keys: true, ..config.storage };
            config.with_storage(storage)
        })
        .with_tree_config_modifier(|config| {
            config.with_persistence_threshold(0).with_memory_block_buffer_target(0)
        })
        .build_single()
        .await?;
    {
        let provider = memcmp.inner.provider.database_provider_ro()?;
        let settings = StorageSettings::v2().with_memcmp_trie_keys(true);
        assert_eq!(provider.storage_settings()?, Some(settings));
        assert_eq!(get_db_version(provider.storage_path())?, MEMCMP_TRIE_KEYS_DB_VERSION);
    }

    let mut deployer = wallet.account(0);
    let contract = deployer.next_contract_address();
    let mut txs = vec![deployer.deploy(STORE_INIT_CODE).await];
    for i in 0..12 {
        txs.push(deployer.transfer(Address::repeat_byte(0x20 + i), U256::from(1)).await);
    }
    let mined = packed.mine(txs).await?.ensure_success()?;
    memcmp.import_payload(mined.payload).await?;

    // Each account sends 12 transactions per block, below the pool's limit per sender. With 120
    // slots the storage trie of the contract has branch nodes below its root.
    let mut accounts = (1..=10).map(|i| wallet.account(i)).collect::<Vec<_>>();
    let mut txs = Vec::new();
    for (i, account) in (0..).zip(&mut accounts) {
        for slot in i * 12..(i + 1) * 12 {
            txs.push(account.call(contract, store_input(slot, slot + 1)).await);
        }
    }
    let mined = packed.mine(txs).await?.ensure_success()?;
    memcmp.import_payload(mined.payload).await?;
    assert_same_tries(&packed, &memcmp, 2).await?;

    // The restarted node reads its trie tables with the encoding stored in its database. The block
    // deletes half of the slots and writes as many new ones.
    let memcmp = memcmp.restart().await?;
    let mut txs = Vec::new();
    for (i, account) in (0..).zip(&mut accounts) {
        for slot in i * 12..i * 12 + 6 {
            txs.push(account.call(contract, store_input(slot, 0)).await);
        }
        for slot in 120 + i * 6..120 + (i + 1) * 6 {
            txs.push(account.call(contract, store_input(slot, slot + 1)).await);
        }
    }
    let mined = packed.mine(txs).await?.ensure_success()?;
    memcmp.import_payload(mined.payload).await?;
    assert_same_tries(&packed, &memcmp, 3).await?;

    Ok(())
}

/// Returns the calldata of a [`STORE_INIT_CODE`] contract call that stores `value` at `slot`.
fn store_input(slot: u64, value: u64) -> Bytes {
    [B256::from(U256::from(slot)), B256::from(U256::from(value))].concat().into()
}

/// Waits until both nodes persisted block `number`, checks each trie against the hashed state of
/// its node, and asserts that both nodes store the same trie nodes.
async fn assert_same_tries(
    packed: &NodeHelperType<EthereumNode>,
    memcmp: &NodeHelperType<EthereumNode>,
    number: u64,
) -> eyre::Result<()> {
    for node in [packed, memcmp] {
        node.wait_for_persisted_block(number).await?;
        assert_trie_consistency(&node.inner.provider)?;
    }

    let packed = packed.inner.provider.database_provider_ro()?;
    let memcmp = memcmp.inner.provider.database_provider_ro()?;
    let accounts = rows::<PackedAccountsTrie>(packed.tx_ref())?
        .into_iter()
        .map(|(path, node)| (path.0, node))
        .collect::<Vec<_>>();
    assert!(!accounts.is_empty());
    assert_eq!(
        accounts,
        rows::<MemcmpAccountsTrie>(memcmp.tx_ref())?
            .into_iter()
            .map(|(path, node)| (path.0, node))
            .collect::<Vec<_>>()
    );

    let storages = rows::<PackedStoragesTrie>(packed.tx_ref())?
        .into_iter()
        .map(|(address, entry)| (address, entry.nibbles.0, entry.node))
        .collect::<Vec<_>>();
    assert!(!storages.is_empty());
    assert_eq!(
        storages,
        rows::<MemcmpStoragesTrie>(memcmp.tx_ref())?
            .into_iter()
            .map(|(address, entry)| (address, entry.nibbles.0, entry.node))
            .collect::<Vec<_>>()
    );
    Ok(())
}

/// Returns all rows of table `T`, decoding every key and value.
fn rows<T: Table>(tx: &impl DbTx) -> eyre::Result<Vec<(T::Key, T::Value)>> {
    Ok(tx.cursor_read::<T>()?.walk(None)?.collect::<Result<_, _>>()?)
}
