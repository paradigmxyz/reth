use super::*;
use commonware_runtime::{deterministic, Runner, Supervisor};
use reth_trie::{
    hashed_cursor::{noop::NoopHashedCursor, HashedPostStateCursor},
    proof::Proof,
    trie_cursor::noop::{NoopAccountTrieCursor, NoopStorageTrieCursor},
    HashedPostStateSorted, HashedStorage,
};

/// Models a flat state with no cached branch nodes. The real overlay cursors and proof calculators
/// must reconstruct the account and storage tries; the sequential oracle uses a different encoder.
#[derive(Clone)]
struct Fixture {
    state: Arc<HashedPostStateSorted>,
    active: Arc<AtomicUsize>,
    fail: Arc<AtomicBool>,
}

impl Fixture {
    fn new() -> Self {
        let mut state = HashedPostState::default();
        for index in 1..=4u8 {
            let address = B256::repeat_byte(index);
            state.accounts.insert(
                address,
                Some(Account {
                    nonce: u64::from(index),
                    balance: U256::from(index) * U256::from(100),
                    bytecode_hash: None,
                }),
            );
            state.storages.insert(
                address,
                HashedStorage::from_iter(
                    (1..=3u8).map(|slot| {
                        (B256::repeat_byte(slot), U256::from(index) * U256::from(slot))
                    }),
                ),
            );
        }
        Self {
            state: Arc::new(state.into_sorted()),
            active: Arc::new(AtomicUsize::new(0)),
            fail: Arc::new(AtomicBool::new(false)),
        }
    }

    fn oracle(&self) -> DecodedMultiProofV2 {
        self.oracle_for(targets())
    }

    fn oracle_for(&self, targets: MultiProofTargetsV2) -> DecodedMultiProofV2 {
        let provider = self.database_provider_ro().unwrap();
        Proof::new(&provider, &provider).multiproof_v2(targets).unwrap()
    }
}

impl DatabaseProviderROFactory for Fixture {
    type Provider = FixtureProvider;

    fn database_provider_ro(&self) -> ProviderResult<Self::Provider> {
        if self.fail.load(Ordering::Acquire) {
            return Err(ProviderError::other(std::io::Error::other(
                "injected proof provider failure",
            )));
        }
        self.active.fetch_add(1, Ordering::AcqRel);
        Ok(FixtureProvider(self.clone()))
    }
}

struct FixtureProvider(Fixture);

impl Drop for FixtureProvider {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::AcqRel);
    }
}

impl TrieCursorFactory for FixtureProvider {
    type AccountTrieCursor<'a> = NoopAccountTrieCursor;
    type StorageTrieCursor<'a> = NoopStorageTrieCursor;

    fn account_trie_cursor(&self) -> Result<Self::AccountTrieCursor<'_>, DatabaseError> {
        Ok(NoopAccountTrieCursor::default())
    }

    fn storage_trie_cursor(
        &self,
        _hashed_address: B256,
    ) -> Result<Self::StorageTrieCursor<'_>, DatabaseError> {
        Ok(NoopStorageTrieCursor::default())
    }
}

impl HashedCursorFactory for FixtureProvider {
    type AccountCursor<'a> = HashedPostStateCursor<'a, NoopHashedCursor<Account>, Option<Account>>;
    type StorageCursor<'a> = HashedPostStateCursor<'a, NoopHashedCursor<U256>, U256>;

    fn hashed_account_cursor(&self) -> Result<Self::AccountCursor<'_>, DatabaseError> {
        Ok(HashedPostStateCursor::new_account(NoopHashedCursor::default(), &self.0.state))
    }

    fn hashed_storage_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageCursor<'_>, DatabaseError> {
        Ok(HashedPostStateCursor::new_storage(
            NoopHashedCursor::default(),
            &self.0.state,
            hashed_address,
        ))
    }
}

fn targets() -> MultiProofTargetsV2 {
    MultiProofTargetsV2 {
        account_targets: [1, 2, 4].map(|index| ProofV2Target::new(B256::repeat_byte(index))).into(),
        // Account 3 has storage targets without an account target. Account 4 needs its storage
        // root without an explicit storage proof, exercising the synchronous encoder fallback.
        storage_targets: [3, 1, 2]
            .into_iter()
            .map(|index| {
                (
                    B256::repeat_byte(index),
                    [1, 3].map(|slot| ProofV2Target::new(B256::repeat_byte(slot))).into(),
                )
            })
            .collect(),
    }
}

fn input(sender: ProofResultSender, nonce: u64) -> AccountMultiproofInput {
    let mut state = HashedPostState::default();
    state.accounts.insert(B256::ZERO, Some(Account { nonce, ..Default::default() }));
    AccountMultiproofInput {
        targets: targets(),
        proof_result_sender: ProofResultContext::new(sender, state, Instant::now()),
    }
}

async fn receive<T>(receiver: &CrossbeamReceiver<T>, runtime: &TaskRuntime) -> T {
    let deadline = runtime.now() + Duration::from_secs(5);
    loop {
        match receiver.try_recv() {
            Ok(message) => return message,
            Err(crossbeam_channel::TryRecvError::Disconnected) => panic!("proof result lost"),
            Err(crossbeam_channel::TryRecvError::Empty) => {
                assert!(runtime.now() < deadline, "proof worker stalled");
                runtime.sleep(Duration::from_millis(1)).await;
            }
        }
    }
}

async fn exercise(
    runtime: &TaskRuntime,
    fixture: &Fixture,
    workers: ProofWorkerHandle,
) -> (Vec<DecodedMultiProofV2>, Vec<u64>) {
    let expected = fixture.oracle();
    let (sender, receiver) = unbounded();
    for nonce in 1..=3 {
        let mut job = input(sender.clone(), nonce);
        if nonce == 2 {
            // The second proof has no storage dependencies and can overtake the first proof
            // while a separate actor waits for its storage workers.
            job.targets = MultiProofTargetsV2::default();
        }
        workers.dispatch_account_multiproof(job).unwrap();
    }
    // A dropped result consumer must not stop a worker or strand its storage dependencies.
    let (canceled_sender, canceled_receiver) = unbounded();
    workers.dispatch_account_multiproof(input(canceled_sender, 4)).unwrap();
    drop(canceled_receiver);

    let address = B256::repeat_byte(1);
    let (storage_sender, storage_receiver) = unbounded();
    workers
        .dispatch_storage_proof(
            StorageProofInput::new(
                address,
                [1, 3].map(|slot| ProofV2Target::new(B256::repeat_byte(slot))).into(),
                true,
            ),
            storage_sender,
        )
        .unwrap();
    let direct = receive(&storage_receiver, runtime).await.result.unwrap();
    assert_eq!(direct.proof, expected.storage_proofs[&address]);
    assert!(direct.root.is_some());
    let mut results = Vec::new();
    let mut nonces = Vec::new();
    for _ in 0..3 {
        let result = receive(&receiver, runtime).await;
        let nonce = result.state.accounts[&B256::ZERO].unwrap().nonce;
        nonces.push(nonce);
        let proof = result.result.unwrap();
        if nonce == 2 {
            assert_eq!(proof, fixture.oracle_for(MultiProofTargetsV2::default()));
        } else {
            assert_eq!(proof, expected);
        }
        results.push(proof);
    }
    let mut sorted_nonces = nonces.clone();
    sorted_nonces.sort_unstable();
    assert_eq!(sorted_nonces, [1, 2, 3]);
    workers.shutdown().await;
    drop(workers);
    let deadline = runtime.now() + Duration::from_secs(5);
    while fixture.active.load(Ordering::Acquire) != 0 {
        assert!(runtime.now() < deadline, "worker retained a database provider after shutdown");
        runtime.sleep(Duration::from_millis(1)).await;
    }
    (results, nonces)
}

async fn failures_and_shutdown(runtime: &TaskRuntime, fixture: &Fixture) {
    let (sender, receiver) = unbounded();
    let workers = ProofWorkerHandle::new_cooperative(
        runtime,
        ProofTaskCtx::new(fixture.clone()),
        sender.clone(),
    );
    fixture.fail.store(true, Ordering::Release);
    workers.dispatch_account_multiproof(input(sender.clone(), 1)).unwrap();
    assert!(receive(&receiver, runtime).await.result.unwrap_err().to_string().contains("injected"));
    fixture.fail.store(false, Ordering::Release);
    workers.dispatch_account_multiproof(input(sender.clone(), 2)).unwrap();
    assert_eq!(receive(&receiver, runtime).await.result.unwrap(), fixture.oracle());

    // Canceling a shutdown waiter must leave admitted work available to drain on retry,
    // including storage jobs that the account workers have not dispatched yet.
    workers.dispatch_account_multiproof(input(sender.clone(), 3)).unwrap();
    let mut shutdown = Box::pin(workers.shutdown());
    assert!(poll_fn(|cx| Poll::Ready(shutdown.as_mut().poll(cx))).await.is_pending());
    drop(shutdown);
    workers.shutdown().await;
    assert_eq!(receive(&receiver, runtime).await.result.unwrap(), fixture.oracle());
    assert!(workers.dispatch_account_multiproof(input(sender, 4)).is_err());
    assert!(matches!(
        receive(&receiver, runtime).await.result,
        Err(StateRootTaskError::ProofDispatch(_))
    ));
    drop(workers);
    assert_eq!(fixture.active.load(Ordering::Acquire), 0);

    // Dropping the last owner cancels queue drivers and any child CPU handle they own. Await the
    // result channel closing to ensure cancellation has actually been polled by the executor.
    let (sender, receiver) = unbounded();
    let workers = ProofWorkerHandle::new_cooperative(
        runtime,
        ProofTaskCtx::new(fixture.clone()),
        sender.clone(),
    );
    workers.dispatch_account_multiproof(input(sender.clone(), 5)).unwrap();
    drop(sender);
    runtime.yield_now().await;
    drop(workers);
    let deadline = runtime.now() + Duration::from_secs(5);
    loop {
        match receiver.try_recv() {
            Err(crossbeam_channel::TryRecvError::Disconnected) => break,
            _ => {
                assert!(runtime.now() < deadline, "canceled proof driver retained its sender");
                runtime.sleep(Duration::from_millis(1)).await;
            }
        }
    }
    while fixture.active.load(Ordering::Acquire) != 0 {
        // A native CPU closure that has already started cannot be preempted by canceling its
        // driver. It must still finish and release its provider after the result channel closes.
        assert!(runtime.now() < deadline, "canceled proof worker retained a database provider");
        runtime.sleep(Duration::from_millis(1)).await;
    }
}

#[tokio::test]
async fn native_workers_match_sequential_proof_oracle() {
    let fixture = Fixture::new();
    let native = Runtime::test();
    let runtime = TaskRuntime::from(native.clone());
    let (sender, _) = unbounded();
    let workers =
        ProofWorkerHandle::new(&native, ProofTaskCtx::new(fixture.clone()), false, sender);
    exercise(&runtime, &fixture, workers).await;
}

#[tokio::test]
async fn cooperative_workers_on_native_runtime() {
    let fixture = Fixture::new();
    let runtime = TaskRuntime::from(Runtime::test());
    let (sender, _) = unbounded();
    let workers =
        ProofWorkerHandle::new_cooperative(&runtime, ProofTaskCtx::new(fixture.clone()), sender);
    exercise(&runtime, &fixture, workers).await;
    failures_and_shutdown(&runtime, &fixture).await;
}

#[test]
fn deterministic_proof_workers() {
    fn run(seed: u64) -> (String, (Vec<DecodedMultiProofV2>, Vec<u64>)) {
        let fixture = Fixture::new();
        deterministic::Runner::new(
            deterministic::Config::default()
                .with_seed(seed)
                .with_timeout(Some(Duration::from_secs(20))),
        )
        .start(|context| async move {
            let runtime = TaskRuntime::deterministic(context.child("proofs"));
            let (sender, _) = unbounded();
            let workers = ProofWorkerHandle::new_cooperative(
                &runtime,
                ProofTaskCtx::new(fixture.clone()),
                sender,
            );
            let result = exercise(&runtime, &fixture, workers).await;
            failures_and_shutdown(&runtime, &fixture).await;
            (context.auditor().state(), result)
        })
    }
    let seeds: Vec<u64> = match std::env::var("RETH_DST_SEED") {
        Ok(seed) => vec![seed.parse().expect("RETH_DST_SEED must be a u64")],
        Err(std::env::VarError::NotPresent) => (0..16).collect(),
        Err(error) => panic!("invalid RETH_DST_SEED: {error}"),
    };
    let campaign = seeds.len() > 1;
    let mut observed_overtake = false;
    for seed in seeds {
        eprintln!("proof workers DST: seed={seed}");
        let result = run(seed);
        observed_overtake |= result.1 .1 != [1, 2, 3];
        assert_eq!(result, run(seed), "proof worker replay diverged for seed {seed}");
    }
    if campaign {
        assert!(observed_overtake, "proof campaign did not exercise out-of-order completion");
    }
}
