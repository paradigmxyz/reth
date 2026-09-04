use super::*;
use crate::tree::txpool_prewarm::Handle;
use alloy_primitives::{keccak256, Bytes};
use commonware_runtime::{deterministic, Runner, Supervisor};
use reth_provider::{test_utils::MockNodeTypesWithDB, ProviderFactory};
use reth_tasks::Runtime;
use std::{future::poll_fn, sync::atomic::AtomicBool, task::Poll};

type Factory = ProviderFactory<MockNodeTypesWithDB>;
type Prewarmer = Handle<EthPrimitives, Arc<TestFactory>, EthEvmConfig>;
type Counts = (usize, usize, usize);

/// The actual state-provider builder reads database tables. Only provider availability is
/// scripted; account, bytecode and storage reads go through a real genesis database.
struct TestFactory {
    inner: Factory,
    parent: B256,
    enabled: AtomicBool,
}

impl TestFactory {
    fn new(value: u64) -> Self {
        let genesis = serde_json::from_value(serde_json::json!({
            "config": { "chainId": 1 },
            "gasLimit": "0x1c9c380",
            "alloc": {
                "c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0": {
                    "balance": "0x0", "nonce": "0x1", "code": "0x6001545000",
                    "storage": {
                        "0x0000000000000000000000000000000000000000000000000000000000000001":
                        format!("{value:#066x}")
                    }
                }
            }
        }))
        .unwrap();
        let chain = Arc::new(
            reth_chainspec::ChainSpecBuilder::default()
                .chain(reth_chainspec::MAINNET.chain)
                .genesis(genesis)
                .cancun_activated()
                .build(),
        );
        let inner = reth_provider::test_utils::create_test_provider_factory_with_chain_spec(chain);
        let parent = reth_db_common::init::init_genesis(&inner).unwrap();
        Self { inner, parent, enabled: AtomicBool::new(false) }
    }

    fn enable_database_provider(&self) {
        self.enabled.store(true, Ordering::Release);
    }
}

impl DatabaseProviderFactory for TestFactory {
    type DB = <Factory as DatabaseProviderFactory>::DB;
    type Provider = <Factory as DatabaseProviderFactory>::Provider;
    type ProviderRW = <Factory as DatabaseProviderFactory>::ProviderRW;

    fn database_provider_ro(&self) -> reth_provider::ProviderResult<Self::Provider> {
        if !self.enabled.load(Ordering::Acquire) {
            return Err(reth_provider::ProviderError::other(std::io::Error::other(
                "provider unavailable",
            )));
        }
        DatabaseProviderFactory::database_provider_ro(&self.inner)
    }

    fn database_provider_rw(&self) -> reth_provider::ProviderResult<Self::ProviderRW> {
        self.inner.database_provider_rw()
    }
}

fn fixtures() -> [Arc<TestFactory>; 5] {
    [7, 9, 11, 13, 17].map(|value| Arc::new(TestFactory::new(value)))
}

fn code() -> Bytes {
    // PUSH1 1; SLOAD; POP; STOP. The read must populate both bytecode and storage caches.
    [0x60, 0x01, 0x54, 0x50, 0x00].into()
}

fn contract_call() -> PoolTransaction<EthPrimitives> {
    let transaction = TxLegacy {
        gas_limit: 100_000,
        to: TxKind::Call(Address::repeat_byte(0xC0)),
        ..Default::default()
    };
    let hash = B256::repeat_byte(0xC0);
    let signed = TransactionSigned::Legacy(Signed::new_unchecked(
        transaction,
        Signature::test_signature(),
        hash,
    ));
    let sender = Address::repeat_byte(0xAA);
    PoolTransaction { hash, sender, transaction: Recovered::new_unchecked(signed, sender) }
}

fn start(handle: &Prewarmer, provider: &Arc<TestFactory>) -> B256 {
    let parent = provider.parent;
    handle.start(
        parent,
        Default::default(),
        OverlayStateProviderFactory::new(
            Arc::clone(provider),
            reth_storage_overlay::OverlayManager::default().overlay_builder(parent),
        ),
    );
    parent
}

async fn until(runtime: &TaskRuntime, what: &str, condition: impl Fn() -> bool) {
    let deadline = runtime.now() + WAIT_LIMIT;
    while !condition() {
        assert!(runtime.now() < deadline, "timed out waiting for {what}");
        runtime.sleep(POLL_INTERVAL).await;
    }
}

async fn published(runtime: &TaskRuntime, handle: &Prewarmer, parent: B256) -> Snapshot {
    until(runtime, "txpool snapshot", || handle.snapshot(parent).is_some()).await;
    handle.snapshot(parent).unwrap()
}

/// The same source, commands and EVM reads run against the native worker and the cooperative
/// worker. Real database ownership and cold reopen are additionally exercised by the node test.
async fn exercise(
    runtime: &TaskRuntime,
    native: Option<&Runtime>,
    [first_provider, stale_provider, latest_provider, rejected_provider, cancellation_provider]: [Arc<TestFactory>; 5],
) -> (Counts, Counts, Counts) {
    let pool = Arc::new(ScriptedPool::default());
    let source: Arc<dyn Source<EthPrimitives>> = pool.clone();
    let handle = match native {
        Some(native) => Prewarmer::spawn(native, source, EthEvmConfig::mainnet()),
        None => Prewarmer::spawn_with_runtime(runtime, source, EthEvmConfig::mainnet()),
    };
    let observe = handle.snapshot_observer();
    let first = start(&handle, &first_provider);
    until(runtime, "pool head retry", || pool.not_ready.load(Ordering::Relaxed) > 0).await;
    pool.push(first, contract_call());
    until(runtime, "first iterator", || pool.opened.load(Ordering::Relaxed) == 1).await;
    // Provider failure must leave the live iterator's first transaction available for retry.
    runtime.sleep(REFRESH_INTERVAL * 2).await;
    assert!(handle.snapshot(first).is_none());
    first_provider.enable_database_provider();
    let before = published(runtime, &handle, first).await;
    let address = Address::repeat_byte(0xC0);
    let slot = B256::from(U256::from(1));
    assert_eq!(before.storage(address, slot), Some(U256::from(7)));
    assert!(before.bytecode(&keccak256(code())).unwrap().is_some());
    assert_eq!(before.account(&address).unwrap().unwrap().nonce, 1);
    let first_counts = before.entry_counts();

    let pause = handle.pause();
    let second_pause = handle.pause();
    pool.push(first, transfer(0xB1));
    runtime.sleep(REFRESH_INTERVAL * 2).await;
    assert_eq!(handle.snapshot(first).unwrap().entry_counts(), first_counts);
    drop(pause);
    runtime.sleep(REFRESH_INTERVAL * 2).await;
    assert_eq!(handle.snapshot(first).unwrap().entry_counts(), first_counts);
    drop(second_pause);
    until(runtime, "arriving transaction publication", || {
        handle
            .snapshot(first)
            .is_some_and(|snapshot| snapshot.account(&Address::repeat_byte(0xB1)).is_some())
    })
    .await;
    let expanded = handle.snapshot(first).unwrap();
    assert_eq!(expanded.account(&Address::repeat_byte(0xB1)), Some(None));
    assert_eq!(
        before.account(&Address::repeat_byte(0xB1)),
        None,
        "snapshot mutated after publication"
    );
    assert_eq!(before.entry_counts(), first_counts);
    assert_eq!(pool.opened.load(Ordering::Relaxed), 1, "live iterator was reopened");

    // No transaction from a superseded Start may run, even if the pool has work for that head.
    let pause = handle.pause();
    let stale = start(&handle, &stale_provider);
    let latest = start(&handle, &latest_provider);
    stale_provider.enable_database_provider();
    latest_provider.enable_database_provider();
    pool.push(stale, contract_call());
    pool.push(latest, contract_call());
    drop(pause);
    let replaced = published(runtime, &handle, latest).await;
    assert_eq!(replaced.storage(address, slot), Some(U256::from(11)));
    assert_eq!(before.storage(address, slot), Some(U256::from(7)));
    assert_eq!(
        replaced.account(&Address::repeat_byte(0xB1)),
        None,
        "stale cache survived head change"
    );
    assert!(handle.snapshot(first).is_none());
    assert!(handle.snapshot(stale).is_none());
    assert_eq!(pool.opened.load(Ordering::Relaxed), 2);
    assert_eq!(Arc::strong_count(&first_provider), 1);
    assert_eq!(Arc::strong_count(&stale_provider), 1);

    // A canceled shutdown waiter must leave the actual worker available for a later join.
    let mut shutdown = Box::pin(handle.shutdown());
    let _ = poll_fn(|cx| Poll::Ready(shutdown.as_mut().poll(cx))).await;
    drop(shutdown);
    handle.shutdown().await;
    assert_eq!(Arc::strong_count(&pool), 1, "worker retained its source after shutdown");
    assert_eq!(Arc::strong_count(&latest_provider), 1, "worker retained its parent factory");
    start(&handle, &rejected_provider);
    assert_eq!(Arc::strong_count(&rejected_provider), 1, "shutdown admitted new work");
    drop(handle);
    assert_eq!(observe(latest).unwrap().storage(address, slot), Some(U256::from(11)));
    owner_cancellation(runtime, cancellation_provider).await;
    (first_counts, expanded.entry_counts(), replaced.entry_counts())
}

async fn owner_cancellation(runtime: &TaskRuntime, provider: Arc<TestFactory>) {
    let pool = Arc::new(ScriptedPool::default());
    let source: Arc<dyn Source<EthPrimitives>> = pool.clone();
    let handle = Prewarmer::spawn_with_runtime(runtime, source, EthEvmConfig::mainnet());
    let parent = start(&handle, &provider);
    provider.enable_database_provider();
    pool.push(parent, contract_call());
    // Yield while a transaction may be queued/running. Dropping the owner closes commands;
    // the transaction is allowed to finish before the driver drops its resources.
    runtime.yield_now().await;
    drop(handle);
    until(runtime, "canceled worker cleanup", || Arc::strong_count(&pool) == 1).await;
    assert_eq!(Arc::strong_count(&provider), 1);
}

#[tokio::test]
async fn native_and_cooperative_txpool_prewarming_match() {
    let native = Runtime::test();
    let runtime = TaskRuntime::from(native.clone());
    let expected = exercise(&runtime, Some(&native), fixtures()).await;
    assert_eq!(exercise(&runtime, None, fixtures()).await, expected);
}

#[test]
fn deterministic_txpool_prewarming() {
    fn run(seed: u64) -> (String, (Counts, Counts, Counts)) {
        // Database initialization owns native resources and happens before simulation begins.
        let fixtures = fixtures();
        deterministic::Runner::new(
            deterministic::Config::default()
                .with_seed(seed)
                .with_timeout(Some(Duration::from_secs(20))),
        )
        .start(|context| async move {
            let runtime = TaskRuntime::deterministic(context.child("txpool"));
            let result = exercise(&runtime, None, fixtures).await;
            (context.auditor().state(), result)
        })
    }
    let seeds: Vec<u64> = match std::env::var("RETH_DST_SEED") {
        Ok(seed) => vec![seed.parse().expect("RETH_DST_SEED must be a u64")],
        Err(std::env::VarError::NotPresent) => (0..16).collect(),
        Err(error) => panic!("invalid RETH_DST_SEED: {error}"),
    };
    for seed in seeds {
        eprintln!("txpool prewarming DST: seed={seed}");
        assert_eq!(run(seed), run(seed), "txpool prewarming replay diverged for seed {seed}");
    }
}
