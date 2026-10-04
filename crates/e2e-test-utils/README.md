# reth-e2e-test-utils

Harness for end-to-end tests of reth nodes. It launches real nodes in the test process, each with a
temporary database, the RPC and Engine API servers and networking, and hands the test a
`NodeTestContext` per node to produce blocks, reorg, send engine messages, inject transactions and
wait for the node to reach a state. Everything is generic over the node type, so chains built on
reth can use it with their own node, payload and network types.

## Which style to use

Write new tests against `NodeTestContext` (`src/node.rs`), launched with `E2ETestSetupBuilder`.
The test calls helpers in order and asserts in between, so it reads top to bottom and a failure
points at the step that failed.

The `testsuite` module is an action framework: a `TestBuilder` applies a `Setup` and runs a list of
`Action`s against an `Environment` that reaches the nodes over RPC. The engine tree suite
(`crates/engine/tree/tests/e2e-testsuite`) and the execution-apis RPC compatibility suite
(`crates/rpc/rpc-e2e-tests`) are built on it; use it to extend those suites. Its limits:

- The actions call the versioned Engine API over RPC: `engine_forkchoiceUpdatedV3`,
  `engine_getPayloadV3` and `engine_newPayloadV3` (with empty blob versioned hashes), falling back
  to or starting with `V2` in some actions. No action uses `V4` or later.
- The actions generate their own payload attributes with empty withdrawals, a zero parent beacon
  block root and no slot number, so they need payload attributes convertible from Ethereum's.
- `Setup` takes an Ethereum `ChainSpec`, chain import (`TestBuilder::with_setup_and_import`)
  launches `EthereumNode`s only, and `Setup::tree_config` replaces the tree configuration the
  harness derives from the node configuration, except for the cross block cache size.

The engine tree suite runs on Cancun chains, and the RPC compatibility suite imports a prebuilt
chain. See the
[testsuite README](https://github.com/paradigmxyz/reth/blob/main/crates/e2e-test-utils/src/testsuite/README.md)
for the actions.

## Where tests live and how to run them

| Location | Test binary | Contents |
|---|---|---|
| `crates/ethereum/node/tests/e2e/` | `reth-node-ethereum`, `e2e` | Node behaviour, one module per area, listed in `main.rs` |
| `crates/ethereum/node/tests/it/` | `reth-node-ethereum`, `it` | Node builder and launch tests |
| `crates/e2e-test-utils/tests/` | `e2e_testsuite`, `rocksdb` | The harness itself, storage v2 |
| `crates/engine/tree/tests/e2e-testsuite/` | `reth-engine-tree`, `e2e_testsuite` | Engine tree, action framework |
| `crates/rpc/rpc-e2e-tests/` | `reth-rpc-e2e-tests`, `e2e_testsuite` | RPC compatibility, action framework |

CI runs every `e2e_testsuite` binary in the `e2e` workflow and all other test binaries in the
`integration` workflow. `.config/nextest.toml` retries a failing test twice, marks tests slow after
30 seconds and kills them after 60 seconds, except for the `e2e` binary of `reth-node-ethereum`
(killed after 5 minutes) and `e2e_testsuite` binaries (6 minutes).

```bash
cargo nextest run -p reth-node-ethereum --test e2e
# a single test, by exact name
cargo nextest run -p reth-node-ethereum --test e2e -E 'test(=reorg::can_build_forks_and_reorg)'
# with node logs, for tests that call `reth_tracing::init_test_tracing()`
RUST_LOG=info,engine::tree=debug cargo nextest run -p reth-node-ethereum --test e2e --no-capture \
    -E 'test(=reorg::can_build_forks_and_reorg)'
```

## Which helper to use

| I want to | Use |
|---|---|
| **Setup** | |
| Launch one node with every fork up to X active at genesis | `EthereumNode::test_setup_for(EthereumHardfork::X)`, then `build_single()` |
| Launch several connected nodes | `with_num_nodes(n)` and `build()`; each node peers with the previous one, the last also with the first if there are more than two; `with_connect_nodes(false)` to skip |
| Use my own chain spec, e.g. a fork at a later timestamp | `test_chain_spec_builder()` and `E2ETestSetupExt::test_setup(n, chain_spec)`; `test_genesis()` funds the 20 test accounts |
| Launch a node whose payload attributes do not convert from Ethereum's | `E2ETestSetupBuilder::new_with_attributes_generator`; `with_attributes_generator` replaces the generator of any builder |
| Launch a non-default instance of the node type | `with_node`, called with the node index |
| Install an ExEx, extend the RPC modules, add launch hooks, map the add-ons | `with_node_builder_modifier`, which receives a `TestNodeBuilder` |
| Change node settings that have a CLI flag | `with_node_config_modifier`, `with_rpc_modifier` (default: all modules except `testing` over HTTP), `with_pruning` |
| Change engine tree settings | `with_tree_config_modifier`, applied last, on top of the tree config derived from the node config |
| Choose the storage layout | `with_storage_v2(bool)`, defaults to the node default |
| Sync with another backfill, e.g. snap with `--snap.v2` | `with_backfill(builder)`, e.g. `with_backfill(EthereumBackfill::Snap)`; the dev mining launcher currently supports only the default pipeline backfill |
| Let a local miner build the blocks | `with_dev_mining(block_time)`, `map_dev_payload_attributes`; `with_dev_mode` only sets `--dev` |
| **Accounts and transactions** | |
| Sign transactions from a funded account that tracks its nonce | `wallet.account(i)` (a `TestAccount`), then `account.transfer(to, value).await`, `account.call(to, input)` or `account.deploy(init_code)` |
| Set the nonce, gas or fees of one transaction | the `TestTx` setters, e.g. `.nonce(n)`, `.gas_limit(g)`, `.fees(max, priority)`, `.map_request(f)`; per account `with_gas_limit`, `with_fees` |
| Know the address of the next deployment, resync the nonce | `account.next_contract_address()`, `account.sync_nonce(&provider)` |
| Send through an alloy provider | `node.rpc_provider_with_wallet(wallet.signer(i))` |
| Build a blob or EIP-7702 transaction | `TransactionTestContext::tx_with_blobs_bytes`, `set_code_tx_bytes` (nonce 0, see traps) |
| Put a raw transaction into the pool | `node.rpc.inject_tx(raw)` |
| **Producing blocks** | |
| Mine given transactions and get their receipts | `node.mine(txs)`, which fails if the block misses one of them or includes another pool transaction; `mine_including(txs)` allows other pool transactions |
| Mine signed transactions, e.g. of a custom transaction type, that must all succeed | `node.mine_signed(txs)`, which encodes them, mines them like `mine` and fails if one reverted |
| Mine transactions that are already in the pool, e.g. sent through a provider | `node.mine_pooled(hashes)`, with the checks of `mine` and an error for a hash that is not in the pool |
| Produce blocks from the pool | `advance_block()`, `advance_blocks(n)`; `advance_block_synced()` also waits for the pool to process the block |
| Produce blocks until a transaction is included | `advance_until_receipt(hash)` |
| Produce blocks until the pool has no pending transactions | `advance_until_pool_drained()`, which returns the payloads and leaves queued transactions in the pool |
| Produce blocks while a future runs, e.g. `eth_sendRawTransactionSync` | `advance_while(fut)` |
| Build a payload without submitting it | `new_payload()`; on another parent `build_payload_on(parent)` |
| Control block timestamps, e.g. around a fork | `set_next_payload_timestamp(timestamp)` |
| **Forks and reorgs** | |
| Keep imported blocks reorgable | `set_finality(Finality::Keep)` or `Finality::Lag(n)`; the default `Finality::Head` finalizes every block |
| Build a block or a chain on any known block and make it the head | `advance_block_on(parent)`, `advance_fork(parent, n)` |
| Make a known side chain block the head | `reorg_to(hash)` |
| **Engine** | |
| Insert a payload without making it canonical | `submit_payload(payload)`, an error on `INVALID`; `submit_payload_with_status` returns the status |
| Insert a payload and make it the head | `import_payload(payload)`, which follows the finality policy |
| Send an exact forkchoice state or a sealed block, and get typed errors | `node.engine.forkchoice_updated(state)`, `forkchoice_updated_with_attributes`, `new_payload`, `new_payload_from_block` |
| Test Engine API method versions, fork checks, error codes or encoding | `reth_rpc_api::EngineApiClient` on `node.auth_server_handle().http_client()` |
| **Multiple nodes** | |
| Connect two nodes | `a.connect(&mut b)`, done by `build()` unless disabled |
| Give a node the block of another node | `follower.import_payload(payload)`; the parent must be known |
| Let a node download a chain from its peers | `follower.sync_to(hash)`, which makes the block head, safe and finalized; after a forkchoice update of the test's own, `wait_for_head(hash)` |
| **Stopping and restarting** | |
| Make nodes restartable | `with_restartable_nodes()`; opt-in because each node then runs on a runtime of its own, which costs a few threads per node; not combinable with `with_runtime` |
| Stop a node and keep its datadir | `node.stop()`, which returns a `StoppedNode`; `stopped.data_dir()` while it is stopped; dropping it removes the datadir |
| Launch a stopped node again | `stopped.start()`, which returns a new `NodeTestContext` |
| Stop and start in one step | `node.restart()` |
| Notice that the engine of a node exited | `node.take_exit_future()`, which resolves with `Ok` once the node is stopped and with an error if the engine exits on a fatal error |
| **Waits** | |
| Wait for any condition | `wait::poll_until(what, poll)`; `poll_until_with(PollOpts { .. }, ..)` for another timeout or interval |
| Assert that something does not happen | `wait::assert_holds_for(duration, what, check)` |
| Wait for a block, or a pool state | `wait_block(n, hash, wait_finish_checkpoint)`, `wait_for_pool(condition)`, `wait_for_pool_head(hash)` |
| Wait until a block is the head | `wait_for_head(hash)`, which only observes the node; `wait_block` is also satisfied by a canonical block below the head |
| Wait for transactions to enter or leave the pool | `wait_for_pooled(hashes)`, `wait_for_pool_removal(hashes)`, which returns right away for transactions that never entered |
| Wait for persistence or pruning | `wait_for_persisted_block(n)`, `wait_for_prune_checkpoint(segment, n)` |
| **Assertions and inspection** | |
| Inspect a mined block | `MinedBlock`: `block()`, `receipts`, `chain` (the committed `Chain` with its execution outcome), `ensure_success()` |
| Wait for the receipt of a transaction sent through a provider | `PendingTransactionExt::successful_receipt`, `receipt::await_successful_receipts` |
| Query the node over JSON-RPC | `rpc_provider()`, `rpc_provider_with_wallet(w)`, `rpc_provider_for::<Net>()` for other networks |
| Read node state directly | `node.inner.provider`, `node.inner.pool`, `node.current_forkchoice_state()`, `node.block_hash(n)` (panics if unknown) |
| Check that the persisted state and trie are consistent | `trie::assert_trie_consistency(&node.inner.provider)`, after `wait_for_persisted_block` |

## Recipes

Each recipe is a test without its `#[tokio::test]` attribute, which doctests would compile away.
In a test file, add the attribute and call `reth_tracing::init_test_tracing()` first, so `RUST_LOG`
shows the node logs.

Mine transactions, then read their receipts and the execution outcome of the block:

```rust,no_run
use alloy_primitives::{Address, U256};
use reth_chainspec::EthereumHardfork;
use reth_e2e_test_utils::E2ETestSetupExt;
use reth_node_ethereum::EthereumNode;

async fn mines_transfers() -> eyre::Result<()> {
    let (mut node, wallet) =
        EthereumNode::test_setup_for(EthereumHardfork::Cancun).build_single().await?;
    let mut account = wallet.account(0);
    let recipient = Address::with_last_byte(0x77);

    let first = account.transfer(recipient, U256::from(1)).await;
    let second = account.transfer(recipient, U256::from(2)).await;
    let mined = node.mine([first, second]).await?.ensure_success()?;

    assert_eq!(mined.block().number, 1);
    assert_eq!(mined.receipts[1].gas_used, 21_000);
    let recipient_account = mined.chain.execution_outcome().account(&recipient).flatten();
    assert_eq!(recipient_account.map(|account| account.balance), Some(U256::from(3)));
    Ok(())
}
```

Two nodes, where one imports a block of the other and then syncs to its head:

```rust,no_run
use alloy_primitives::{Address, U256};
use reth_chainspec::EthereumHardfork;
use reth_e2e_test_utils::E2ETestSetupExt;
use reth_node_ethereum::EthereumNode;

async fn follower_imports_and_syncs() -> eyre::Result<()> {
    let (mut nodes, wallet) =
        EthereumNode::test_setup_for(EthereumHardfork::Cancun).with_num_nodes(2).build().await?;
    let follower = nodes.pop().unwrap();
    let mut producer = nodes.pop().unwrap();

    // Hand the block to the follower like a consensus client would.
    let tx = wallet.account(0).transfer(Address::with_last_byte(1), U256::from(1)).await;
    let mined = producer.mine([tx]).await?;
    follower.import_payload(mined.payload.clone()).await?;
    assert_eq!(follower.block_hash(1), mined.block().hash());

    // A head the follower does not know: it downloads the blocks from its peer.
    let head = producer.advance_blocks(3).await?.pop().unwrap().block().hash();
    follower.sync_to(head).await?;
    assert_eq!(follower.current_forkchoice_state()?.head_block_hash, head);
    Ok(())
}
```

A reorg that returns a mined transaction to the pool, and a reorg back:

```rust,no_run
use alloy_primitives::{Address, U256};
use reth_chainspec::EthereumHardfork;
use reth_e2e_test_utils::{node::Finality, E2ETestSetupExt};
use reth_node_ethereum::EthereumNode;

async fn reorg_returns_transaction_to_pool() -> eyre::Result<()> {
    let (mut node, wallet) =
        EthereumNode::test_setup_for(EthereumHardfork::Cancun).build_single().await?;
    // Keep genesis finalized, so the blocks above it can be reorged.
    node.set_finality(Finality::Keep);
    let genesis = node.block_hash(0);

    let tx = wallet.account(0).transfer(Address::with_last_byte(1), U256::from(1)).await;
    let mined = node.mine([tx]).await?;
    let (a1, tx_hash) = (mined.block().hash(), mined.receipts[0].transaction_hash);
    // Otherwise the pool may still hold the transaction when the fork is built.
    node.wait_for_pool_head(a1).await?;

    let b1 = node.advance_block_on(genesis).await?.block().hash();
    assert_eq!(node.current_forkchoice_state()?.head_block_hash, b1);
    node.wait_for_pooled([tx_hash]).await?;

    node.reorg_to(a1).await?;
    assert_eq!(node.current_forkchoice_state()?.head_block_hash, a1);
    Ok(())
}
```

A hardfork that activates mid-test:

```rust,no_run
use reth_e2e_test_utils::{test_chain_spec_builder, E2ETestSetupExt};
use reth_node_ethereum::EthereumNode;
use std::sync::Arc;

async fn activates_prague() -> eyre::Result<()> {
    let prague_time = 1_000;
    let chain_spec =
        Arc::new(test_chain_spec_builder().cancun_activated().with_prague_at(prague_time).build());
    let (mut node, _) = EthereumNode::test_setup(1, chain_spec).build_single().await?;

    // Payload timestamps start far after the fork, so move them right before it.
    node.set_next_payload_timestamp(prague_time - 1)?;
    let cancun = node.advance_block().await?;
    let prague = node.advance_block().await?;
    assert_eq!(prague.block().timestamp, prague_time);
    assert!(cancun.block().requests_hash.is_none());
    assert!(prague.block().requests_hash.is_some());
    Ok(())
}
```

An invalid payload. `submit_payload` would return this as an error naming the block, the latest
valid hash and the validation error:

```rust,no_run
use alloy_primitives::B256;
use alloy_rpc_types_engine::PayloadStatusEnum;
use reth_chainspec::EthereumHardfork;
use reth_e2e_test_utils::E2ETestSetupExt;
use reth_node_ethereum::EthereumNode;
use reth_primitives_traits::SealedBlock;

async fn rejects_wrong_state_root() -> eyre::Result<()> {
    let (mut node, _) =
        EthereumNode::test_setup_for(EthereumHardfork::Cancun).build_single().await?;
    let payload = node.new_payload().await?;

    // A wrong state root is only detected after executing the block.
    let mut block = payload.block().clone_block();
    block.header.state_root = B256::ZERO;
    let status = node.engine.new_payload_from_block(SealedBlock::seal_slow(block), None).await?;

    assert_eq!(status.latest_valid_hash, Some(payload.block().parent_hash));
    let validation_error = format!(
        "mismatched block state root: got {}, expected {}",
        payload.block().state_root,
        B256::ZERO
    );
    assert_eq!(status.status, PayloadStatusEnum::Invalid { validation_error });
    Ok(())
}
```

A forkchoice update the engine rejects, with the typed error. Over the Engine API it would be error
code -38002:

```rust,no_run
use alloy_rpc_types_engine::{ForkchoiceState, ForkchoiceUpdateError};
use reth_chainspec::EthereumHardfork;
use reth_e2e_test_utils::{node::Finality, E2ETestSetupExt};
use reth_node_api::BeaconForkChoiceUpdateError;
use reth_node_ethereum::EthereumNode;

async fn rejects_safe_block_above_head() -> eyre::Result<()> {
    let (mut node, _) =
        EthereumNode::test_setup_for(EthereumHardfork::Cancun).build_single().await?;
    node.set_finality(Finality::Keep);
    let genesis = node.block_hash(0);
    let blocks = node.advance_blocks(2).await?;
    let (b1, b2) = (blocks[0].block().hash(), blocks[1].block().hash());

    let state =
        ForkchoiceState { head_block_hash: b1, safe_block_hash: b2, finalized_block_hash: genesis };
    let err = node.engine.forkchoice_updated(state).await.unwrap_err();
    assert!(
        matches!(
            err,
            BeaconForkChoiceUpdateError::ForkchoiceUpdateError(ForkchoiceUpdateError::InvalidState)
        ),
        "{err}"
    );
    assert_eq!(node.current_forkchoice_state()?.head_block_hash, b2);
    Ok(())
}
```

A negative pool assertion, without a fixed sleep:

```rust,no_run
use alloy_primitives::{Address, U256};
use reth_chainspec::EthereumHardfork;
use reth_e2e_test_utils::{wait::assert_holds_for, E2ETestSetupExt};
use reth_node_ethereum::EthereumNode;
use reth_transaction_pool::TransactionPool;
use std::time::Duration;

async fn gapped_transaction_stays_queued() -> eyre::Result<()> {
    let (mut node, wallet) =
        EthereumNode::test_setup_for(EthereumHardfork::Cancun).build_single().await?;

    // Nonce 1 while the account is at nonce 0.
    let tx = wallet.account(0).transfer(Address::with_last_byte(1), U256::from(1)).nonce(1).await;
    let hash = node.rpc.inject_tx(tx).await?;
    // Fails if the block includes any pool transaction.
    node.mine([]).await?;

    let pool = &node.inner.pool;
    assert_holds_for(Duration::from_secs(1), "gapped transaction to stay queued", move || {
        async move { Ok(pool.contains(&hash) && pool.pending_transactions().is_empty()) }
    })
    .await
}
```

Persistence and pruning:

```rust,no_run
use alloy_primitives::{Address, U256};
use reth_chainspec::EthereumHardfork;
use reth_e2e_test_utils::{trie::assert_trie_consistency, E2ETestSetupExt};
use reth_node_core::args::PruningArgs;
use reth_node_ethereum::EthereumNode;
use reth_prune_types::PruneSegment;

async fn persists_and_prunes() -> eyre::Result<()> {
    let (mut node, wallet) = EthereumNode::test_setup_for(EthereumHardfork::Cancun)
        // Persist every canonical block instead of keeping the recent ones in memory.
        .with_tree_config_modifier(|config| {
            config.with_persistence_threshold(0).with_memory_block_buffer_target(0)
        })
        .with_pruning(PruningArgs {
            account_history_distance: Some(5),
            minimum_distance: Some(5),
            block_interval: Some(1),
            ..Default::default()
        })
        .build_single()
        .await?;

    // The account history checkpoint only reaches blocks that changed an account.
    let mut account = wallet.account(0);
    for _ in 0..20 {
        node.mine([account.transfer(Address::with_last_byte(1), U256::from(1)).await]).await?;
    }
    node.wait_for_persisted_block(20).await?;
    node.wait_for_prune_checkpoint(PruneSegment::AccountHistory, 15).await?;
    assert_trie_consistency(&node.inner.provider)?;
    Ok(())
}
```

A restart, after which the node still has its chain and builds on its head:

```rust,no_run
use alloy_primitives::{Address, U256};
use reth_chainspec::EthereumHardfork;
use reth_e2e_test_utils::E2ETestSetupExt;
use reth_node_ethereum::EthereumNode;

async fn restart_keeps_chain() -> eyre::Result<()> {
    let (mut node, wallet) = EthereumNode::test_setup_for(EthereumHardfork::Cancun)
        .with_restartable_nodes()
        .build_single()
        .await?;
    let mut account = wallet.account(0);
    let recipient = Address::with_last_byte(1);
    let head = node.mine([account.transfer(recipient, U256::from(1)).await]).await?.block().hash();

    // Persists the canonical chain, closes the database and launches the node on its datadir.
    let mut node = node.restart().await?;
    assert_eq!(node.block_hash(1), head);
    let next = node.mine([account.transfer(recipient, U256::from(2)).await]).await?;
    assert_eq!(next.block().parent_hash, head);
    Ok(())
}
```

An ExEx installed through the node builder:

```rust,no_run
use futures_util::TryStreamExt;
use reth_chainspec::EthereumHardfork;
use reth_e2e_test_utils::{wait::WAIT_TIMEOUT, E2ETestSetupExt};
use reth_node_ethereum::EthereumNode;
use tokio::sync::mpsc;

async fn exex_receives_committed_chain() -> eyre::Result<()> {
    let (committed_tx, mut committed_rx) = mpsc::unbounded_channel();
    let (mut node, _) = EthereumNode::test_setup_for(EthereumHardfork::Cancun)
        .with_node_builder_modifier(move |builder| {
            let committed_tx = committed_tx.clone();
            builder.install_exex("committed-chains", |mut ctx| async move {
                Ok(async move {
                    while let Some(notification) = ctx.notifications.try_next().await? {
                        if let Some(chain) = notification.committed_chain() {
                            ctx.send_finished_height(chain.tip().num_hash())?;
                            let _ = committed_tx.send(chain);
                        }
                    }
                    Ok(())
                })
            })
        })
        .build_single()
        .await?;

    let payload = node.advance_block().await?;
    let chain = tokio::time::timeout(WAIT_TIMEOUT, committed_rx.recv())
        .await?
        .ok_or_else(|| eyre::eyre!("ExEx stopped before receiving a committed chain"))?;
    assert_eq!(chain.tip().hash(), payload.block().hash());
    Ok(())
}
```

A node with its own payload attributes generator, here to set the fee recipient:

```rust,no_run
use alloy_primitives::Address;
use alloy_rpc_types_engine::PayloadAttributes;
use reth_chainspec::EthereumHardfork;
use reth_e2e_test_utils::{eth_payload_attributes, test_chain_spec, E2ETestSetupBuilder};
use reth_node_ethereum::EthereumNode;

async fn builds_with_fee_recipient() -> eyre::Result<()> {
    let chain_spec = test_chain_spec(EthereumHardfork::Cancun);
    let attributes_chain_spec = chain_spec.clone();
    let fee_recipient = Address::with_last_byte(0x42);
    let (mut node, _) = E2ETestSetupBuilder::<EthereumNode>::new_with_attributes_generator(
        1,
        chain_spec,
        move |timestamp| PayloadAttributes {
            suggested_fee_recipient: fee_recipient,
            ..eth_payload_attributes(&attributes_chain_spec, timestamp)
        },
    )
    .build_single()
    .await?;

    let payload = node.advance_block().await?;
    assert_eq!(payload.block().header().beneficiary, fee_recipient);
    Ok(())
}
```

## Traps

- **`update_forkchoice(current_head, new_head)`** makes its first argument the safe and finalized
  block, not the previous head, and returns the engine's response without checking it, so a
  `SYNCING` or `INVALID` update does not fail the test. Use `import_payload`, `reorg_to`, or
  `node.engine.forkchoice_updated` with an explicit `ForkchoiceState` and assert on the result. If
  the update makes the node sync, wait for the block with `wait_for_head`: `wait_block` can return
  before the engine made a backfilled block its head.
- **`new_payload` only builds.** It starts a payload job and resolves it; the engine has not seen
  the block. `build_and_submit_payload` inserts it without making it canonical.
- **Every imported block is finalized by default.** Under `Finality::Head`, `import_payload` and
  every helper built on it make the new block safe and finalized, and the engine refuses to reorg
  below the finalized block, so `reorg_to`, `advance_block_on` and `build_payload_on` fail on older
  blocks with an error naming the finalized block. Call `set_finality(Finality::Keep)` before
  producing the blocks to reorg. `sync_to` and `update_forkchoice` ignore the policy.
- **The pool lags behind the chain.** `advance_block`, `mine` and `import_payload` return once the
  block is canonical, but the pool processes it in the background, so right afterwards it can still
  hold the mined transactions and old nonces, and a block built next, e.g. on a fork, can include
  them. Use `advance_block_synced` or `wait_for_pool_head(hash)`. These only cover the pool's own
  update; a node that runs additional pool maintenance tasks on new blocks, e.g. for a separate
  sub-pool or its own eviction rules, can report the new head before they ran, so wait for the
  expected contents with `wait_for_pool_removal` or `wait_for_pooled` instead. After a reorg, the
  transactions of the reorged blocks return to the pool later still; wait for them with
  `wait_for_pooled`.
- **`canonical_stream` is a backlog.** It holds every canonical notification since the context was
  created, up to 256 unread ones (older ones are dropped without an error). `mine`,
  `mine_including`, `inject_and_advance` and `advance` skip ahead to the notification of their
  block; nothing else reads it. After a few `advance_block` calls, `canonical_stream.next()`
  returns the oldest unread notification, not the latest. Prefer `MinedBlock::chain`.
- **The harness bypasses the Engine API RPC layer.** `node.engine` and all block producing and
  importing helpers send messages to the engine in-process: no check that the method version
  matches the fork, no check of fork-specific fields, no JSON or SSZ decoding, no error codes. Test
  those with `EngineApiClient` on the auth server.
- **`TransactionTestContext` signs with nonce 0** (except `transfer_tx_bytes_with_nonce`) and fixed
  fees, so a second transaction from the same signer conflicts with the first. Use `TestAccount`,
  which tracks nonces. `wallet.inner` is the signer of account 0.
- **Dev mining nodes drive themselves.** With `with_dev_mining`, the local miner sends the
  forkchoice updates and builds the blocks, so the block producing and forkchoice helpers of
  `NodeTestContext` must not be used, and only a single node can be launched. Wait for receipts
  with `successful_receipt`.
- **Payload timestamps do not start at genesis.** The first payload gets timestamp 1710338136,
  or the timestamp of the latest block plus one if that is later, so a fork scheduled at a smaller
  timestamp is active from the first block. Use `set_next_payload_timestamp`.
- **Persistence is lazy, and the waits are narrow.** The engine only persists once more than
  `persistence_threshold` (default 50) canonical blocks are in memory, and keeps
  `memory_block_buffer_target` (default 5) of them; lower both with `with_tree_config_modifier` in
  tests that read the database. `wait_for_persisted_block(n)` returns once the database tip reaches
  `n`: it does not check the block hash, the state of the most recently persisted blocks can still
  lag unless `num_state_masking_blocks` or the persistence threshold is 0, and pruning happens in a
  later commit, see `wait_for_prune_checkpoint`. The account history checkpoint is the last pruned
  block that changed an account, so it can stay below the pruning target if the last blocks in
  the range changed none, e.g. empty blocks.
- **`stop` needs the database to itself.** `stop` and `restart` consume the context and wait until
  the harness holds the last handle of the node's database. A clone of `inner.provider`, a
  `NodeClient` from `to_node_client`, or a handle of `inner.provider.rocksdb_provider()` makes them
  fail after `node::DATABASE_RELEASE_TIMEOUT` (10 seconds), an open `database_provider_ro`
  transaction right away. Drop those first. Payloads, `MinedBlock` with its `chain`, clones of
  `node.engine` and alloy providers do not hold the database.
- **A restart keeps the chain, not the context.** Stopping persists every canonical block and saves
  the local transactions of the pool, which the node reinserts when it starts; blocks that are not
  canonical, e.g. only submitted or reorged out, are lost. Unless it mines in dev mode, the
  restarted node gets a forkchoice update that restates the head, safe and finalized blocks it
  persisted. It also gets a new `NodeTestContext`: its finality policy is `Finality::Head` again,
  and its `canonical_stream` only sees notifications from the restart on.
- **A restarted node does not reconnect.** It keeps its peer id but listens on new ports, and test
  nodes do not persist their peers. Connect it with `restarted.connect(&mut peer)` once the peer
  noticed the disconnect, e.g. once `peer.inner.network.num_connected_peers()` dropped.
  `peer.connect(&mut restarted)` panics, because the network events of the peer still hold the
  disconnect.

## Rules for new tests

- No fixed sleeps. Wait for the condition with `poll_until` or a `wait_for_*` helper, and assert
  that something does not happen with `assert_holds_for`.
- Set nodes up with `E2ETestSetupBuilder`. Use `NodeBuilder` directly only to test the builder
  itself, as `crates/ethereum/node/tests/it` does. If the setup builder can't express a setup, add
  the option to it.
- Assert exact errors: compare the whole error message or match the typed error, not `is_err()` or
  a substring.
- Test one behaviour per test, and name the test after it.
- If a helper is missing, add it to the harness instead of a copy in the test: node steps to
  `NodeTestContext` (`src/node.rs`), waits to `src/wait.rs`, transaction building to `TestAccount`
  and `TestTx` (`src/wallet.rs`), setup options to `E2ETestSetupBuilder` (`src/setup_builder.rs`),
  engine messages to `EngineTestContext` (`src/engine.rs`). Document what the helper guarantees
  and add its future to `test_helper_futures_are_send`.
