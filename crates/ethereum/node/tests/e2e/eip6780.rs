//! Post-Cancun SELFDESTRUCT (EIP-6780) scenario suite driven through the engine API.
//!
//! The same scenario battery runs against multiple hardfork targets (Cancun, Osaka, Amsterdam)
//! on a single node per fork: every block goes through payload building, `newPayload` and
//! forkchoice via the consensus engine, and assertions cover receipts, RPC state and the
//! committed execution outcome. At the end of a suite the persisted database is checked with
//! [`assert_trie_consistency`], which recomputes the state root from the on-disk hashed state
//! and verifies the stored trie nodes against it, so incorrect persistence of selfdestructed
//! accounts (such as the prefunded `CREATE2` divergence introduced by
//! <https://github.com/bluealloy/revm/pull/3863>) is caught even when the live, in-memory state
//! root is correct.
//!
//! The scenarios cover the EIP-6780 equivalence classes that affect reth's state
//! representation:
//! - prefunded `CREATE2` target destroyed in its creation transaction (prior block and same block
//!   prefunding),
//! - creation and destruction in the same transaction, both directly from initcode and via a call
//!   after the contract returned its runtime code,
//! - creation and destruction in different transactions of the same block (must be treated as
//!   pre-existing),
//! - selfdestruct to self for pre-existing (balance kept) and same-transaction created (balance
//!   burned, or kept as a balance-only account once EIP-8246 is active) contracts, and to a
//!   contract beneficiary (code must not run),
//! - a reverted nested selfdestruct while the outer frame commits,
//! - destroy/recreate cycles at the same `CREATE2` address across transactions and blocks,
//!   including the collision of a recreate attempt with a persisting destroyed contract.

use alloy_network::{EthereumWallet, TransactionBuilder};
use alloy_primitives::{Address, Bytes, TxKind, B256, U256};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_rpc_types_eth::{TransactionReceipt, TransactionRequest};
use futures::StreamExt;
use reth_chainspec::{ChainSpec, ChainSpecBuilder, EthereumHardfork, MAINNET};
use reth_e2e_test_utils::{
    eth_payload_attributes_for_fork, setup_engine,
    trie::{assert_trie_consistency, wait_for_persisted_block},
    NodeHelperType,
};
use reth_node_api::TreeConfig;
use reth_node_ethereum::EthereumNode;
use reth_provider::Chain;
use reth_revm::db::BundleAccount;
use std::{sync::Arc, time::Duration};

const MAX_FEE_PER_GAS: u128 = 20_000_000_000;
const MAX_PRIORITY_FEE_PER_GAS: u128 = 1_000_000_000;

const ETH: u128 = 1_000_000_000_000_000_000;

#[tokio::test]
async fn test_eip6780_selfdestruct_cancun() -> eyre::Result<()> {
    run_selfdestruct_suite(EthereumHardfork::Cancun).await
}

#[tokio::test]
async fn test_eip6780_selfdestruct_osaka() -> eyre::Result<()> {
    run_selfdestruct_suite(EthereumHardfork::Osaka).await
}

#[tokio::test]
async fn test_eip6780_selfdestruct_amsterdam() -> eyre::Result<()> {
    run_selfdestruct_suite(EthereumHardfork::Amsterdam).await
}

/// Runs all selfdestruct scenarios back to back against a single node with the given hardfork
/// active at genesis, then verifies the persisted trie representation.
async fn run_selfdestruct_suite(fork: EthereumHardfork) -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    // low persistence thresholds so the scenario blocks reach the database and the persisted
    // trie representation can be verified at the end of the suite
    let tree_config =
        TreeConfig::default().with_persistence_threshold(2).with_memory_block_buffer_target(1);
    let (mut nodes, wallet) =
        setup_engine::<EthereumNode>(1, fork_spec(fork), false, tree_config, move |timestamp| {
            eth_payload_attributes_for_fork(fork, timestamp)
        })
        .await?;
    let node = nodes.pop().unwrap();
    let signer = wallet.inner.clone();
    let provider = ProviderBuilder::new()
        .wallet(EthereumWallet::new(signer.clone()))
        .connect_http(node.rpc_url());

    let mut ctx = SuiteCtx {
        node,
        provider,
        fork,
        signer: signer.address(),
        nonce: 0,
        factory: Address::ZERO,
        last_committed: None,
    };

    let fixtures = deploy_fixtures(&mut ctx).await?;

    prefunded_create2_selfdestruct(&mut ctx).await?;
    same_block_prefunded_create2_selfdestruct(&mut ctx, &fixtures).await?;
    create_then_destroy_in_same_tx(&mut ctx).await?;
    create_and_destroy_in_different_txs_of_same_block(&mut ctx).await?;
    selfdestruct_to_self(&mut ctx, &fixtures).await?;
    reverted_selfdestruct(&mut ctx, &fixtures).await?;
    destroy_and_recreate(&mut ctx).await?;
    destroy_and_recreate_in_same_block(&mut ctx).await?;

    // leave the scenario blocks behind the in-memory window so they are persisted, then verify
    // the on-disk state and trie representation against the persisted tip
    let scenario_tip = ctx.provider.get_block_number().await?;
    for _ in 0..4 {
        ctx.mine_block(vec![]).await?;
    }
    wait_for_persisted_block(&ctx.node.inner.provider, scenario_tip, Duration::from_secs(30))
        .await?;
    assert_trie_consistency(&ctx.node.inner.provider)?;

    Ok(())
}

/// Test context that drives a single node and tracks the signer nonce across scenarios.
struct SuiteCtx<P> {
    node: NodeHelperType<EthereumNode>,
    provider: P,
    fork: EthereumHardfork,
    signer: Address,
    nonce: u64,
    /// The mode-switched `CREATE2` factory all scenarios deploy through.
    factory: Address,
    /// The chain segment committed by the most recently mined block.
    last_committed: Option<Arc<Chain>>,
}

impl<P: Provider> SuiteCtx<P> {
    /// Sends the given transactions from the signer and mines a block containing exactly them.
    ///
    /// Nonces and fees are filled in automatically in transaction order.
    async fn mine_block(
        &mut self,
        txs: Vec<TransactionRequest>,
    ) -> eyre::Result<Vec<TransactionReceipt>> {
        let expected = txs.len();
        let mut pending = Vec::with_capacity(expected);
        for tx in txs {
            let nonce = self.nonce;
            self.nonce += 1;
            let tx = tx
                .with_from(self.signer)
                .with_nonce(nonce)
                .with_max_fee_per_gas(MAX_FEE_PER_GAS)
                .with_max_priority_fee_per_gas(MAX_PRIORITY_FEE_PER_GAS);
            pending.push(self.provider.send_transaction(tx).await?);
        }

        let payload = self.node.advance_block().await?;
        let included = payload.block().body().transactions().count();
        assert_eq!(included, expected, "block should contain exactly the sent transactions");

        let notification = self
            .node
            .canonical_stream
            .next()
            .await
            .ok_or_else(|| eyre::eyre!("canonical stream ended"))?;
        self.last_committed = Some(notification.committed());

        let mut receipts = Vec::with_capacity(expected);
        for tx in pending {
            receipts.push(tx.get_receipt().await?);
        }
        for (index, receipt) in receipts.iter().enumerate() {
            assert!(
                receipt.status(),
                "transaction {index} in block {:?} reverted: gas used {}",
                receipt.block_number,
                receipt.gas_used,
            );
        }
        Ok(receipts)
    }

    /// Returns the account entry of the most recently committed block's bundle state.
    fn bundle_account(&self, address: Address) -> Option<&BundleAccount> {
        self.last_committed.as_ref().unwrap().execution_outcome().bundle.account(&address)
    }

    async fn balance(&self, address: Address) -> eyre::Result<U256> {
        Ok(self.provider.get_balance(address).await?)
    }

    async fn slot0(&self, address: Address) -> eyre::Result<U256> {
        Ok(self.provider.get_storage_at(address, U256::ZERO).await?)
    }

    /// Asserts that the account is fully absent: no balance, code, nonce or storage.
    async fn assert_destroyed(&self, address: Address, scenario: &str) -> eyre::Result<()> {
        assert_eq!(self.balance(address).await?, U256::ZERO, "{scenario}: balance not cleared");
        assert!(
            self.provider.get_code_at(address).await?.is_empty(),
            "{scenario}: code not cleared"
        );
        assert_eq!(
            self.provider.get_transaction_count(address).await?,
            0,
            "{scenario}: nonce not cleared"
        );
        assert_eq!(self.slot0(address).await?, U256::ZERO, "{scenario}: storage not cleared");
        Ok(())
    }

    /// Asserts that a live contract has the expected runtime code, slot 0 value and balance.
    async fn assert_contract(
        &self,
        address: Address,
        code: &Bytes,
        slot0: U256,
        balance: U256,
        scenario: &str,
    ) -> eyre::Result<()> {
        assert_eq!(&self.provider.get_code_at(address).await?, code, "{scenario}: wrong code");
        assert_eq!(self.slot0(address).await?, slot0, "{scenario}: wrong storage");
        assert_eq!(self.balance(address).await?, balance, "{scenario}: wrong balance");
        Ok(())
    }

    /// Predicts the address the factory deploys the given initcode to.
    fn create2_address(&self, salt: B256, initcode: &Bytes) -> Address {
        self.factory.create2_from_code(salt, initcode)
    }
}

/// Contracts shared between scenarios, deployed in the first block of the suite.
struct Fixtures {
    /// Pre-existing contract whose runtime selfdestructs to itself, funded with 1 ETH.
    self_destructing_to_self: Address,
    /// Pre-existing contract whose runtime selfdestructs to `beneficiary(7)`, funded with 1 ETH.
    revert_child: Address,
    /// Contract that calls `revert_child` and then stores to prove its own frame committed,
    /// swallowing the revert of the intermediate [`call_then_revert_runtime`] contract.
    ///
    /// Also serves as the contract beneficiary in `same_block_prefunded_create2_selfdestruct`,
    /// which asserts that its slot 0 is still zero and therefore must run before
    /// `reverted_selfdestruct` permanently sets it.
    revert_wrapper: Address,
}

/// Deploys the `CREATE2` factory and the fixture contracts for the pre-existing and revert
/// scenarios.
async fn deploy_fixtures<P: Provider>(ctx: &mut SuiteCtx<P>) -> eyre::Result<Fixtures> {
    let base_nonce = ctx.nonce;
    ctx.factory = ctx.signer.create(base_nonce);
    let self_destructing_to_self = ctx.signer.create(base_nonce + 1);
    let revert_child = ctx.signer.create(base_nonce + 2);
    let reverter = ctx.signer.create(base_nonce + 3);
    let revert_wrapper = ctx.signer.create(base_nonce + 4);

    let receipts = ctx
        .mine_block(vec![
            create_tx(deploying_initcode(&factory_runtime()), U256::ZERO),
            create_tx(deploying_initcode(&selfdestruct_to_self_runtime()), eth_tenths(10)),
            create_tx(deploying_initcode(&selfdestruct_runtime(beneficiary(7))), eth_tenths(10)),
            create_tx(deploying_initcode(&call_then_revert_runtime(revert_child)), U256::ZERO),
            create_tx(deploying_initcode(&call_then_store_runtime(reverter)), U256::ZERO),
        ])
        .await?;
    assert_eq!(receipts[0].contract_address, Some(ctx.factory));

    Ok(Fixtures { self_destructing_to_self, revert_child, revert_wrapper })
}

/// The incident reproducer: an address is prefunded in an earlier block, then a
/// contract is `CREATE2`-deployed there whose initcode selfdestructs. The account had prior
/// state, so its deletion must be represented correctly all the way into the persisted trie.
async fn prefunded_create2_selfdestruct<P: Provider>(ctx: &mut SuiteCtx<P>) -> eyre::Result<()> {
    let initcode = selfdestruct_initcode(beneficiary(1));
    let destroyed = ctx.create2_address(salt(1), &initcode);

    let receipts = ctx.mine_block(vec![transfer_tx(destroyed, eth_tenths(10))]).await?;
    assert_eq!(ctx.balance(destroyed).await?, eth_tenths(10));
    // EIP-7708: plain transfers emit a transfer log once Amsterdam is active
    assert_eq!(
        !receipts[0].inner.logs().is_empty(),
        ctx.fork >= EthereumHardfork::Amsterdam,
        "transfer log emission must match EIP-7708 activation"
    );

    ctx.mine_block(vec![factory_create_tx(ctx.factory, false, salt(1), &initcode, eth_tenths(5))])
        .await?;

    assert_eq!(ctx.slot0(ctx.factory).await?, address_word(destroyed), "creation should succeed");
    ctx.assert_destroyed(destroyed, "prefunded create2").await?;
    assert_eq!(ctx.balance(beneficiary(1)).await?, eth_tenths(15));
    // the account existed in the database before this block, so the bundle must mark it as
    // destroyed with no post-block info
    let account = ctx.bundle_account(destroyed).expect("prefunded account must be in the bundle");
    assert!(account.was_destroyed(), "prefunded create2: bundle must mark account destroyed");
    assert!(account.info.is_none(), "prefunded create2: destroyed account must have no info");

    // touching the address again must start from a clean account
    ctx.mine_block(vec![transfer_tx(destroyed, eth_tenths(3))]).await?;
    assert_eq!(ctx.balance(destroyed).await?, eth_tenths(3), "old balance must not resurface");
    assert!(ctx.provider.get_code_at(destroyed).await?.is_empty());

    Ok(())
}

/// Same as [`prefunded_create2_selfdestruct`], but the prefunding happens in an earlier
/// transaction of the same block, and the selfdestruct beneficiary is a contract whose code
/// must not be executed by the balance transfer.
async fn same_block_prefunded_create2_selfdestruct<P: Provider>(
    ctx: &mut SuiteCtx<P>,
    fixtures: &Fixtures,
) -> eyre::Result<()> {
    let contract_beneficiary = fixtures.revert_wrapper;
    let initcode = selfdestruct_initcode(contract_beneficiary);
    let destroyed = ctx.create2_address(salt(2), &initcode);

    ctx.mine_block(vec![
        transfer_tx(destroyed, eth_tenths(10)),
        factory_create_tx(ctx.factory, false, salt(2), &initcode, eth_tenths(3)),
    ])
    .await?;

    assert_eq!(ctx.slot0(ctx.factory).await?, address_word(destroyed), "creation should succeed");
    ctx.assert_destroyed(destroyed, "same-block prefunded create2").await?;
    // the beneficiary contract receives the funds without its code running
    assert_eq!(ctx.balance(contract_beneficiary).await?, eth_tenths(13));
    assert_eq!(ctx.slot0(contract_beneficiary).await?, U256::ZERO, "beneficiary code must not run");

    Ok(())
}

/// A contract is `CREATE2`-deployed, returns its runtime code and is then called to
/// selfdestruct later in the same transaction: it still counts as created in this transaction
/// and must be deleted.
async fn create_then_destroy_in_same_tx<P: Provider>(ctx: &mut SuiteCtx<P>) -> eyre::Result<()> {
    let initcode = deploying_initcode(&selfdestruct_runtime(beneficiary(3)));
    let destroyed = ctx.create2_address(salt(3), &initcode);

    ctx.mine_block(vec![factory_create_tx(ctx.factory, true, salt(3), &initcode, eth_tenths(2))])
        .await?;

    assert_eq!(ctx.slot0(ctx.factory).await?, address_word(destroyed), "creation should succeed");
    ctx.assert_destroyed(destroyed, "create then destroy in same tx").await?;
    assert_eq!(ctx.balance(beneficiary(3)).await?, eth_tenths(2));

    Ok(())
}

/// A contract created in an earlier transaction of the block selfdestructs in a later
/// transaction of the same block: it must be treated as pre-existing, transferring only the
/// balance while account, code and storage persist. A repeated selfdestruct with zero balance
/// in the next block must leave it untouched as well.
async fn create_and_destroy_in_different_txs_of_same_block<P: Provider>(
    ctx: &mut SuiteCtx<P>,
) -> eyre::Result<()> {
    let runtime = selfdestruct_runtime(beneficiary(4));
    let initcode = deploying_initcode(&runtime);
    let contract = ctx.create2_address(salt(4), &initcode);

    ctx.mine_block(vec![
        factory_create_tx(ctx.factory, false, salt(4), &initcode, eth_tenths(2)),
        call_tx(contract),
    ])
    .await?;

    ctx.assert_contract(contract, &runtime, eth_tenths(2), U256::ZERO, "same-block create").await?;
    assert_eq!(ctx.provider.get_transaction_count(contract).await?, 1, "nonce must persist");
    assert_eq!(ctx.balance(beneficiary(4)).await?, eth_tenths(2));
    let account = ctx.bundle_account(contract).expect("contract must be in the bundle");
    assert!(!account.was_destroyed(), "same-block create: account must not be destroyed");
    assert!(account.info.is_some(), "same-block create: account must persist");

    // repeated selfdestruct of the now pre-existing contract with zero balance
    ctx.mine_block(vec![call_tx(contract)]).await?;
    ctx.assert_contract(contract, &runtime, eth_tenths(2), U256::ZERO, "repeated selfdestruct")
        .await?;
    assert_eq!(ctx.balance(beneficiary(4)).await?, eth_tenths(2), "no double transfer");

    Ok(())
}

/// Selfdestruct with the beneficiary being the destructing account itself: a pre-existing
/// contract keeps its balance, while a contract created in the same transaction burns it.
async fn selfdestruct_to_self<P: Provider>(
    ctx: &mut SuiteCtx<P>,
    fixtures: &Fixtures,
) -> eyre::Result<()> {
    let initcode = selfdestruct_to_self_initcode();
    let destroyed = ctx.create2_address(salt(6), &initcode);

    ctx.mine_block(vec![
        call_tx(fixtures.self_destructing_to_self),
        factory_create_tx(ctx.factory, false, salt(6), &initcode, eth_tenths(4)),
    ])
    .await?;

    // the pre-existing contract keeps its funds: the transfer to itself is a no-op
    ctx.assert_contract(
        fixtures.self_destructing_to_self,
        &selfdestruct_to_self_runtime(),
        eth_tenths(10),
        eth_tenths(10),
        "pre-existing selfdestruct to self",
    )
    .await?;

    assert_eq!(ctx.slot0(ctx.factory).await?, address_word(destroyed), "creation should succeed");
    if ctx.fork >= EthereumHardfork::Amsterdam {
        // EIP-8246: instead of burning the funds, the account survives as a balance-only
        // account without nonce, code or storage
        ctx.assert_contract(
            destroyed,
            &Bytes::new(),
            U256::ZERO,
            eth_tenths(4),
            "same-tx selfdestruct to self",
        )
        .await?;
        assert_eq!(ctx.provider.get_transaction_count(destroyed).await?, 0, "nonce must be reset");
    } else {
        // the same-transaction created contract is deleted and its funds are burned
        ctx.assert_destroyed(destroyed, "same-tx selfdestruct to self").await?;
    }

    Ok(())
}

/// An inner frame selfdestructs a pre-existing contract, but the intermediate frame reverts
/// while the outer frame commits: the selfdestruct must be rolled back completely.
async fn reverted_selfdestruct<P: Provider>(
    ctx: &mut SuiteCtx<P>,
    fixtures: &Fixtures,
) -> eyre::Result<()> {
    ctx.mine_block(vec![call_tx(fixtures.revert_wrapper)]).await?;

    // the outer frame committed
    assert_eq!(
        ctx.slot0(fixtures.revert_wrapper).await?,
        U256::ONE,
        "reverted selfdestruct: outer frame must commit"
    );
    // the reverted selfdestruct left the child completely untouched
    ctx.assert_contract(
        fixtures.revert_child,
        &selfdestruct_runtime(beneficiary(7)),
        eth_tenths(10),
        eth_tenths(10),
        "reverted selfdestruct",
    )
    .await?;
    assert_eq!(ctx.balance(beneficiary(7)).await?, U256::ZERO, "transfer must be reverted");
    if let Some(account) = ctx.bundle_account(fixtures.revert_child) {
        assert!(!account.was_destroyed(), "reverted selfdestruct: account must not be destroyed");
    }

    Ok(())
}

/// Destroy/recreate cycle at the same `CREATE2` address across blocks: an address freed by a
/// same-transaction destruction can be recreated with fresh storage, the recreated contract is
/// pre-existing for a later selfdestruct, and afterwards the address keeps colliding with
/// creation attempts.
async fn destroy_and_recreate<P: Provider>(ctx: &mut SuiteCtx<P>) -> eyre::Result<()> {
    let runtime = selfdestruct_runtime(beneficiary(8));
    let initcode = deploying_initcode(&runtime);
    let contract = ctx.create2_address(salt(8), &initcode);
    let factory_balance_before = ctx.balance(ctx.factory).await?;

    // create and destroy in the same transaction to free the address again
    ctx.mine_block(vec![factory_create_tx(ctx.factory, true, salt(8), &initcode, eth_tenths(1))])
        .await?;
    ctx.assert_destroyed(contract, "destroy before recreate").await?;

    // recreate at the same address; the initcode stores the callvalue, so a value distinct
    // from the first deployment proves the storage is fresh
    ctx.mine_block(vec![factory_create_tx(ctx.factory, false, salt(8), &initcode, eth_tenths(2))])
        .await?;
    ctx.assert_contract(contract, &runtime, eth_tenths(2), eth_tenths(2), "recreate").await?;

    // the recreated contract is pre-existing now: selfdestruct keeps code and storage
    ctx.mine_block(vec![call_tx(contract)]).await?;
    ctx.assert_contract(contract, &runtime, eth_tenths(2), U256::ZERO, "destroy recreated").await?;
    assert_eq!(ctx.balance(beneficiary(8)).await?, eth_tenths(3));

    // the destroyed but persisting contract keeps colliding with creation attempts; the failed
    // `CREATE2` leaves the value with the factory
    ctx.mine_block(vec![factory_create_tx(ctx.factory, false, salt(8), &initcode, eth_tenths(1))])
        .await?;
    assert_eq!(ctx.slot0(ctx.factory).await?, U256::ZERO, "creation must collide");
    assert_eq!(ctx.balance(ctx.factory).await?, factory_balance_before + eth_tenths(1));
    ctx.assert_contract(
        contract,
        &runtime,
        eth_tenths(2),
        U256::ZERO,
        "collision with destroyed contract",
    )
    .await?;

    Ok(())
}

/// Destroy and recreate at the same `CREATE2` address within one block: the first transaction
/// creates and destroys the contract, the second recreates it at the now free address.
async fn destroy_and_recreate_in_same_block<P: Provider>(
    ctx: &mut SuiteCtx<P>,
) -> eyre::Result<()> {
    let runtime = selfdestruct_runtime(beneficiary(9));
    let initcode = deploying_initcode(&runtime);
    let contract = ctx.create2_address(salt(9), &initcode);

    ctx.mine_block(vec![
        factory_create_tx(ctx.factory, true, salt(9), &initcode, eth_tenths(1)),
        factory_create_tx(ctx.factory, false, salt(9), &initcode, eth_tenths(3)),
    ])
    .await?;

    assert_eq!(ctx.slot0(ctx.factory).await?, address_word(contract), "recreation should succeed");
    ctx.assert_contract(
        contract,
        &runtime,
        eth_tenths(3),
        eth_tenths(3),
        "same-block destroy and recreate",
    )
    .await?;
    assert_eq!(ctx.balance(beneficiary(9)).await?, eth_tenths(1));

    Ok(())
}

// Transaction builders
//
// All transactions use a generous fixed gas limit: Amsterdam repricing raises costs well above
// the historic values (a plain transfer no longer fits into 21k gas), and gas estimation cannot
// be used because some transactions depend on contracts created earlier in the same block.

const TX_GAS_LIMIT: u64 = 1_000_000;

fn create_tx(initcode: Bytes, value: U256) -> TransactionRequest {
    TransactionRequest::default()
        .with_kind(TxKind::Create)
        .with_input(initcode)
        .with_value(value)
        .with_gas_limit(TX_GAS_LIMIT)
}

fn call_tx(to: Address) -> TransactionRequest {
    TransactionRequest::default().with_to(to).with_gas_limit(TX_GAS_LIMIT)
}

fn transfer_tx(to: Address, value: U256) -> TransactionRequest {
    TransactionRequest::default().with_to(to).with_value(value).with_gas_limit(TX_GAS_LIMIT)
}

/// Calls the factory to `CREATE2`-deploy the given initcode, optionally calling the deployed
/// contract afterwards in the same transaction.
fn factory_create_tx(
    factory: Address,
    call_after_create: bool,
    salt: B256,
    initcode: &Bytes,
    value: U256,
) -> TransactionRequest {
    let mut input = Vec::with_capacity(64 + initcode.len());
    input.extend_from_slice(&U256::from(call_after_create as u8).to_be_bytes::<32>());
    input.extend_from_slice(salt.as_slice());
    input.extend_from_slice(initcode);
    TransactionRequest::default()
        .with_to(factory)
        .with_input(input)
        .with_value(value)
        .with_gas_limit(TX_GAS_LIMIT)
}

// Scenario parameters and small conversion helpers

fn eth_tenths(tenths: u128) -> U256 {
    U256::from(tenths * ETH / 10)
}

const fn salt(id: u8) -> B256 {
    B256::with_last_byte(id)
}

/// Returns a distinct EOA beneficiary address per scenario.
const fn beneficiary(id: u8) -> Address {
    let mut bytes = [0xbe; 20];
    bytes[19] = id;
    Address::new(bytes)
}

/// The address as it appears in a storage slot written by the factory.
fn address_word(address: Address) -> U256 {
    U256::from_be_slice(address.as_slice())
}

// Contract bytecode, hand-assembled to keep the scenarios self-contained

/// Initcode that stores a marker at slot 0 and selfdestructs to `target` during initialization.
fn selfdestruct_initcode(target: Address) -> Bytes {
    let mut code = vec![0x60, 0x42, 0x60, 0x00, 0x55]; // PUSH1 0x42 PUSH1 0 SSTORE
    code.push(0x73); // PUSH20
    code.extend_from_slice(target.as_slice());
    code.push(0xff); // SELFDESTRUCT
    code.into()
}

/// Initcode that stores a marker at slot 0 and selfdestructs to its own address during
/// initialization.
const fn selfdestruct_to_self_initcode() -> Bytes {
    // PUSH1 0x42 PUSH1 0 SSTORE ADDRESS SELFDESTRUCT
    Bytes::from_static(&[0x60, 0x42, 0x60, 0x00, 0x55, 0x30, 0xff])
}

/// Runtime code that selfdestructs to `target` on any call.
fn selfdestruct_runtime(target: Address) -> Bytes {
    let mut code = vec![0x73]; // PUSH20
    code.extend_from_slice(target.as_slice());
    code.push(0xff); // SELFDESTRUCT
    code.into()
}

/// Runtime code that selfdestructs to its own address on any call.
const fn selfdestruct_to_self_runtime() -> Bytes {
    // ADDRESS SELFDESTRUCT
    Bytes::from_static(&[0x30, 0xff])
}

/// Initcode that stores the callvalue at slot 0 and returns the given runtime code.
///
/// Storing the callvalue makes redeployments distinguishable: the same initcode deployed with a
/// different value must yield different storage, proving that no stale storage survived.
fn deploying_initcode(runtime: &Bytes) -> Bytes {
    // runtime starts after the 16 initcode bytes below
    let offset = 0x10_u8;
    let len = u8::try_from(runtime.len()).unwrap();
    let mut code = vec![
        0x34, 0x60, 0x00, 0x55, // CALLVALUE PUSH1 0 SSTORE
        0x60, len, 0x60, offset, 0x60, 0x00, 0x39, // PUSH1 len PUSH1 offset PUSH1 0 CODECOPY
        0x60, len, 0x60, 0x00, 0xf3, // PUSH1 len PUSH1 0 RETURN
    ];
    code.extend_from_slice(runtime);
    code.into()
}

/// Runtime of the `CREATE2` factory.
///
/// Calldata layout: `[mode word | salt | initcode]`. The factory forwards its callvalue to
/// `CREATE2` and stores the resulting address (zero on failure) at slot 0. A non-zero mode word
/// additionally calls the deployed contract with empty calldata, which lets a single
/// transaction create a contract and trigger its selfdestruct after initialization.
const fn factory_runtime() -> Bytes {
    const CODE: [u8; 44] = [
        0x60, 0x40, 0x36, 0x03, // PUSH1 0x40 CALLDATASIZE SUB -> initcode length
        0x80, 0x60, 0x40, 0x60, 0x00, 0x37, // DUP1 PUSH1 0x40 PUSH1 0 CALLDATACOPY
        0x60, 0x20, 0x35, 0x90, // PUSH1 0x20 CALLDATALOAD SWAP1 -> salt below length
        0x60, 0x00, 0x34, 0xf5, // PUSH1 0 CALLVALUE CREATE2 -> deployed address
        0x80, 0x60, 0x00, 0x55, // DUP1 PUSH1 0 SSTORE -> record address at slot 0
        0x60, 0x00, 0x35, // PUSH1 0 CALLDATALOAD -> mode word
        0x60, 0x1d, 0x57, // PUSH1 0x1d JUMPI -> call section if mode is non-zero
        0x00, // STOP
        0x5b, // JUMPDEST (offset 0x1d)
        0x60, 0x00, 0x60, 0x00, 0x60, 0x00, 0x60, 0x00, 0x60, 0x00, // ret/args/value zeros
        0x85, 0x5a, 0xf1, // DUP6 GAS CALL -> call the deployed address
        0x00, // STOP
    ];
    const _: () = assert!(CODE[0x1d] == 0x5b, "jump destination must point at the JUMPDEST");
    Bytes::from_static(&CODE)
}

/// Runtime that calls `target` (whose runtime selfdestructs) and then reverts, undoing the
/// selfdestruct.
fn call_then_revert_runtime(target: Address) -> Bytes {
    let mut code = vec![0x60, 0x00, 0x60, 0x00, 0x60, 0x00, 0x60, 0x00, 0x60, 0x00];
    code.push(0x73); // PUSH20
    code.extend_from_slice(target.as_slice());
    code.extend_from_slice(&[0x5a, 0xf1, 0x50]); // GAS CALL POP
    code.extend_from_slice(&[0x60, 0x00, 0x60, 0x00, 0xfd]); // PUSH1 0 PUSH1 0 REVERT
    code.into()
}

/// Runtime that calls `target`, ignores the result and stores 1 at slot 0 to prove that its own
/// frame committed.
fn call_then_store_runtime(target: Address) -> Bytes {
    let mut code = vec![0x60, 0x00, 0x60, 0x00, 0x60, 0x00, 0x60, 0x00, 0x60, 0x00];
    code.push(0x73); // PUSH20
    code.extend_from_slice(target.as_slice());
    code.extend_from_slice(&[0x5a, 0xf1, 0x50]); // GAS CALL POP
    code.extend_from_slice(&[0x60, 0x01, 0x60, 0x00, 0x55]); // PUSH1 1 PUSH1 0 SSTORE
    code.push(0x00); // STOP
    code.into()
}

fn fork_spec(fork: EthereumHardfork) -> Arc<ChainSpec> {
    let builder = ChainSpecBuilder::default()
        .chain(MAINNET.chain)
        .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap());
    let builder = match fork {
        EthereumHardfork::Cancun => builder.cancun_activated(),
        EthereumHardfork::Osaka => builder.osaka_activated(),
        EthereumHardfork::Amsterdam => builder.amsterdam_activated(),
        fork => unimplemented!("no activation configured for {fork}"),
    };
    Arc::new(builder.build())
}
