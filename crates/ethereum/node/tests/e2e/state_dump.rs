use alloy_consensus::constants::EMPTY_ROOT_HASH;
use alloy_eips::BlockId;
use alloy_genesis::{Genesis, GenesisAccount};
use alloy_primitives::{keccak256, Address, Bytes, B256, U256};
use alloy_provider::Provider;
use eyre::{eyre, Result};
use reth_chainspec::{ChainSpecBuilder, MAINNET};
use reth_node_builder::{NodeBuilder, NodeHandle};
use reth_node_core::{args::RpcServerArgs, node_config::NodeConfig};
use reth_node_ethereum::EthereumNode;
use reth_rpc_api::HashedStateDump;
use reth_rpc_server_types::RpcModuleSelection;
use reth_tasks::Runtime;
use std::{collections::BTreeMap, sync::Arc};

/// Storage slots of the account that still fits into a single dump.
const SMALL_STORAGE_SLOTS: u64 = 128;

/// Storage slots of the account that exceeds the dump's storage budget of 16384 slots.
const HUGE_STORAGE_SLOTS: u64 = 20_000;

/// Number of plain (storage-less) accounts allocated in genesis.
const PLAIN_ACCOUNTS: u64 = 5;

/// Accounts per page requested while paging through the entire state.
const PAGE_SIZE: u64 = 3;

/// Exercises `debug_accountRange` and `debug_dumpBlock` against a genesis that allocates an
/// account with more storage than a single dump is allowed to read.
#[tokio::test]
async fn debug_account_range_dumps_genesis_state() -> Result<()> {
    reth_tracing::init_test_tracing();

    let plain_accounts = (0..PLAIN_ACCOUNTS)
        .map(|i| {
            (
                account_address(i),
                GenesisAccount::default()
                    .with_balance(U256::from(i + 1))
                    .with_nonce(Some(i))
                    .with_code(Some(account_code(i))),
            )
        })
        .collect::<Vec<_>>();

    let small_storage_address = account_address(PLAIN_ACCOUNTS);
    let small_storage_account = GenesisAccount::default()
        .with_balance(U256::from(1_000))
        .with_code(Some(account_code(PLAIN_ACCOUNTS)))
        .with_storage(Some(storage(SMALL_STORAGE_SLOTS)));

    // Dumping this account's storage must exceed the budget, so its hash is picked to sort last
    // and the dump ends before it rather than on it.
    let huge_storage_address = last_sorting_address();
    let huge_storage_account = GenesisAccount::default()
        .with_balance(U256::from(2_000))
        .with_storage(Some(storage(HUGE_STORAGE_SLOTS)));

    let mut genesis = Genesis::default();
    genesis.alloc.extend(plain_accounts.iter().cloned());
    genesis.alloc.insert(small_storage_address, small_storage_account.clone());
    genesis.alloc.insert(huge_storage_address, huge_storage_account.clone());

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(genesis)
            .cancun_activated()
            .build(),
    );

    let node_config = NodeConfig::test().with_chain(chain_spec).with_rpc(
        RpcServerArgs::default()
            .with_unused_ports()
            .with_http()
            .with_http_api(RpcModuleSelection::all_modules().into()),
    );

    let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(node_config)
        .testing_node(Runtime::test())
        .node(EthereumNode::default())
        .launch()
        .await?;

    let provider = node.rpc_server_handle().eth_http_provider().unwrap();
    let genesis_root = provider
        .get_block(BlockId::latest())
        .await?
        .ok_or_else(|| eyre!("missing genesis block"))?
        .header
        .state_root;

    // Page through the whole state, without storage, to learn the full account set.
    let mut all_accounts = Vec::new();
    let mut start = Bytes::new();
    loop {
        let dump: HashedStateDump = provider
            .client()
            .request("debug_accountRange", (BlockId::latest(), start, PAGE_SIZE, true, true, false))
            .await?;

        assert_eq!(dump.root, genesis_root);
        assert!(dump.accounts.len() as u64 <= PAGE_SIZE, "page exceeds maxResults");
        all_accounts.extend(dump.accounts);

        let Some(next) = dump.next else { break };
        assert_eq!(next.len(), 32, "next must be a hashed address");
        start = next;
    }

    let hashes = all_accounts.iter().map(|(hash, _)| *hash).collect::<Vec<_>>();
    assert!(hashes.windows(2).all(|pair| pair[0] < pair[1]), "accounts must be strictly ascending");

    let huge_storage_hash = keccak256(huge_storage_address);
    assert_eq!(
        hashes.last(),
        Some(&huge_storage_hash),
        "the account with huge storage must sort last"
    );

    // Accounts are keyed by their hashed address, and carry no address preimage.
    for (address, account) in &plain_accounts {
        let hashed_address = keccak256(address);
        let dumped = all_accounts
            .iter()
            .find_map(|(hash, dumped)| (*hash == hashed_address).then_some(dumped))
            .ok_or_else(|| eyre!("account {address} missing from dump"))?;

        assert_eq!(dumped.balance, account.balance);
        assert_eq!(Some(dumped.nonce), account.nonce);
        assert_eq!(Some(dumped.code_hash), account.code_hash());
        assert_eq!(dumped.root, EMPTY_ROOT_HASH);
        assert_eq!(dumped.address, None);
        assert_eq!(dumped.address_hash, Some(hashed_address));
        assert_eq!(dumped.code, None, "nocode was requested");
        assert_eq!(dumped.storage, None, "nostorage was requested");
    }

    // A single account, with its storage and code.
    let dump: HashedStateDump = provider
        .client()
        .request(
            "debug_accountRange",
            (
                BlockId::latest(),
                Bytes::from(keccak256(small_storage_address).0),
                1,
                false,
                false,
                false,
            ),
        )
        .await?;

    let (hashed_address, dumped) =
        dump.accounts.first_key_value().ok_or_else(|| eyre!("account range is empty"))?;
    assert_eq!(*hashed_address, keccak256(small_storage_address));
    assert_eq!(dumped.code, small_storage_account.code);
    assert_eq!(
        dumped.storage.as_ref().map(|storage| storage.len()),
        Some(SMALL_STORAGE_SLOTS as usize),
    );

    // Storage slots are keyed by hashed slot, for the same reason accounts are.
    let expected_storage = storage(SMALL_STORAGE_SLOTS)
        .into_iter()
        .map(|(slot, value)| (keccak256(slot), U256::from_be_bytes(value.0)))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(dumped.storage.as_ref(), Some(&expected_storage));

    let proof = provider.get_proof(small_storage_address, Vec::new()).await?;
    assert_eq!(dumped.root, proof.storage_hash);

    // Reading the huge account's storage would blow the budget, so it is refused outright rather
    // than dumped partially.
    let err = provider
        .client()
        .request::<_, HashedStateDump>(
            "debug_accountRange",
            (BlockId::latest(), Bytes::from(huge_storage_hash.0), 1, false, false, false),
        )
        .await
        .expect_err("dumping an account over the storage budget must fail");
    assert!(err.to_string().contains("nostorage"), "unexpected error: {err}");

    // Without storage it dumps fine, storage root included.
    let dump: HashedStateDump = provider
        .client()
        .request(
            "debug_accountRange",
            (BlockId::latest(), Bytes::from(huge_storage_hash.0), 1, false, true, false),
        )
        .await?;
    let dumped = dump.accounts.get(&huge_storage_hash).ok_or_else(|| eyre!("account missing"))?;
    assert_eq!(dumped.storage, None);
    assert_eq!(dumped.balance, huge_storage_account.balance);

    let proof = provider.get_proof(huge_storage_address, Vec::new()).await?;
    assert_eq!(dumped.root, proof.storage_hash);
    assert_ne!(dumped.root, EMPTY_ROOT_HASH);

    // `debug_dumpBlock` dumps a single page with storage, so it stops before the huge account and
    // points at it.
    let dump: HashedStateDump =
        provider.client().request("debug_dumpBlock", (BlockId::latest(),)).await?;

    assert_eq!(dump.root, genesis_root);
    assert_eq!(dump.next, Some(Bytes::from(huge_storage_hash.0)));
    assert_eq!(
        dump.accounts.keys().copied().collect::<Vec<_>>(),
        hashes[..hashes.len() - 1].to_vec(),
        "every account but the huge one must be dumped"
    );
    assert_eq!(
        dump.accounts[&keccak256(small_storage_address)].storage.as_ref(),
        Some(&expected_storage)
    );

    Ok(())
}

/// Returns a deterministic address for the given index.
fn account_address(index: u64) -> Address {
    Address::from_slice(&keccak256(index.to_be_bytes())[..20])
}

/// Returns deterministic, non-empty code for the given index.
fn account_code(index: u64) -> Bytes {
    Bytes::from(vec![0x60, 0x00, 0x60, index as u8, 0x55])
}

/// Returns an address whose hash sorts after every other genesis account's.
fn last_sorting_address() -> Address {
    (0u64..)
        .map(account_address)
        .find(|address| keccak256(address)[0] == 0xff)
        .expect("address with a 0xff-prefixed hash")
}

/// Returns `slots` deterministic, non-zero storage entries.
fn storage(slots: u64) -> BTreeMap<B256, B256> {
    (0..slots)
        .map(|slot| (B256::from(U256::from(slot)), B256::from(U256::from(slot + 1))))
        .collect()
}
