//! Cooperative queue drivers around the production proof calculators.

use super::*;
use reth_tasks::{TaskError, TaskHandle};
use std::{future::poll_fn, pin::Pin, sync::Mutex, task::Poll};

#[cfg(test)]
mod tests;

/// Owns the queue drivers without retaining their database factory in the join handles.
#[derive(Debug)]
pub(super) struct CooperativeProofTasks {
    runtime: TaskRuntime,
    stop_accounts: Arc<AtomicBool>,
    stop_storage: Arc<AtomicBool>,
    admission: Mutex<()>,
    handles: Mutex<Option<[Vec<TaskHandle<()>>; 2]>>,
}

impl CooperativeProofTasks {
    pub(super) fn send<T>(
        &self,
        sender: &CrossbeamSender<T>,
        job: T,
    ) -> Result<(), crossbeam_channel::SendError<T>> {
        let _guard = self.admission.lock().expect("proof admission lock poisoned");
        if self.stop_accounts.load(Ordering::Acquire) {
            Err(crossbeam_channel::SendError(job))
        } else {
            sender.send(job)
        }
    }

    pub(super) async fn shutdown(&self) {
        {
            // Serialize admission with closing, including when this cooperative driver is used
            // on the production runtime and submission races with shutdown on another thread.
            let _guard = self.admission.lock().expect("proof admission lock poisoned");
            self.stop_accounts.store(true, Ordering::Release);
        }
        loop {
            // Keep handles in the owner while waiting, so canceling this shutdown future leaves
            // them available to a later caller instead of aborting work that must be drained.
            let finished = poll_fn(|cx| {
                let mut handles = self.handles.lock().expect("proof task lock poisoned");
                let Some([accounts, storage]) = handles.as_mut() else {
                    return Poll::Ready(true);
                };
                accounts.retain_mut(|task| Pin::new(task).poll(cx).is_pending());
                if !accounts.is_empty() {
                    return Poll::Ready(false);
                }
                // Account proofs may enqueue storage dependencies until they finish. Drain the
                // entire account group before allowing idle storage workers to stop.
                self.stop_storage.store(true, Ordering::Release);
                storage.retain_mut(|task| Pin::new(task).poll(cx).is_pending());
                let finished = storage.is_empty();
                if finished {
                    *handles = None;
                }
                Poll::Ready(finished)
            })
            .await;
            if finished {
                return;
            }
            // Separate timers also support concurrent shutdown callers: join handles store only
            // the most recently registered waker, so each waiter must be able to make progress.
            self.runtime.sleep(Duration::from_millis(1)).await;
        }
    }
}

pub(super) fn spawn<Factory>(
    runtime: &TaskRuntime,
    task_ctx: ProofTaskCtx<Factory>,
) -> ProofWorkerHandle
where
    Factory: DatabaseProviderROFactory<Provider: TrieCursorFactory + HashedCursorFactory>
        + Clone
        + Send
        + Sync
        + 'static,
{
    // Separate actors allow independent proofs to finish out of queue order, as with the native
    // worker pools. Fixed counts and spawn order make the simulated schedule reproducible.
    const WORKERS: usize = 2;
    let (storage_work_tx, storage_work_rx) = unbounded();
    let (account_work_tx, account_work_rx) = unbounded();
    let storage_availability = Arc::new(AvailabilitySheet::new(WORKERS));
    let account_availability = Arc::new(AvailabilitySheet::new(WORKERS));
    let cached_storage_roots = Arc::<DashMap<_, _>>::default();
    let stop_accounts = Arc::new(AtomicBool::new(false));
    let stop_storage = Arc::new(AtomicBool::new(false));
    let mut storage = Vec::with_capacity(WORKERS);
    let mut accounts = Vec::with_capacity(WORKERS);
    for worker_index in 0..WORKERS {
        storage.push(
            runtime
                .spawn(
                    "storage_proof_driver",
                    run_storage(
                        runtime.clone(),
                        task_ctx.clone(),
                        storage_work_rx.clone(),
                        Arc::clone(&storage_availability),
                        Arc::clone(&cached_storage_roots),
                        Arc::clone(&stop_storage),
                        worker_index,
                    ),
                )
                .abort_on_drop(),
        );
    }
    for worker_index in 0..WORKERS {
        accounts.push(
            runtime
                .spawn(
                    "account_proof_driver",
                    run_accounts(
                        runtime.clone(),
                        task_ctx.clone(),
                        account_work_rx.clone(),
                        storage_work_tx.clone(),
                        Arc::clone(&account_availability),
                        Arc::clone(&cached_storage_roots),
                        Arc::clone(&stop_accounts),
                        worker_index,
                    ),
                )
                .abort_on_drop(),
        );
    }
    ProofWorkerHandle {
        storage_work_tx,
        account_work_tx,
        storage_availability,
        account_availability,
        storage_worker_count: WORKERS,
        account_worker_count: WORKERS,
        cooperative: Some(Arc::new(CooperativeProofTasks {
            runtime: runtime.clone(),
            stop_accounts,
            stop_storage,
            admission: Mutex::new(()),
            handles: Mutex::new(Some([accounts, storage])),
        })),
    }
}

async fn next_job<T>(
    receiver: &CrossbeamReceiver<T>,
    stopped: &AtomicBool,
    runtime: &TaskRuntime,
) -> Option<T> {
    loop {
        match receiver.try_recv() {
            Ok(job) => return Some(job),
            Err(crossbeam_channel::TryRecvError::Disconnected) => return None,
            Err(crossbeam_channel::TryRecvError::Empty) => {
                if stopped.load(Ordering::Acquire) {
                    return None;
                }
                runtime.sleep(Duration::from_millis(1)).await;
            }
        }
    }
}

async fn run_storage<Factory>(
    runtime: TaskRuntime,
    task_ctx: ProofTaskCtx<Factory>,
    receiver: CrossbeamReceiver<StorageWorkerJob>,
    availability: Arc<AvailabilitySheet>,
    cached_storage_roots: Arc<DashMap<B256, B256>>,
    stopped: Arc<AtomicBool>,
    worker_index: usize,
) where
    Factory: DatabaseProviderROFactory<Provider: TrieCursorFactory + HashedCursorFactory>
        + Clone
        + Send
        + Sync
        + 'static,
{
    availability.mark_idle(worker_index);
    while let Some(StorageWorkerJob::StorageProof { input, proof_result_sender }) =
        next_job(&receiver, &stopped, &runtime).await
    {
        availability.mark_busy(worker_index);
        #[cfg(feature = "trie-debug")]
        if let Some(delay) = task_ctx.proof_jitter {
            // Random OS sleeps are not suitable for this backend. The configured maximum acts
            // as an explicit virtual delay; scheduling variation comes from the runner seed.
            runtime.sleep(delay).await;
        }
        let hashed_address = input.hashed_address;
        let factory = task_ctx.factory.clone();
        let result = runtime
            .spawn_cpu("storage_proof", move || storage_proof(factory, input))
            .abort_on_drop()
            .await
            .unwrap_or_else(|error| {
                Err(StateProofError::Database(DatabaseError::Other(error.to_string())))
            });
        if let Some(root) = result.as_ref().ok().and_then(StorageProofResult::root) {
            cached_storage_roots.insert(hashed_address, root);
        }
        let _ = proof_result_sender.send(StorageProofResultMessage { hashed_address, result });
        availability.mark_idle(worker_index);
        runtime.yield_now().await;
    }
}

#[expect(clippy::too_many_arguments)]
async fn run_accounts<Factory>(
    runtime: TaskRuntime,
    task_ctx: ProofTaskCtx<Factory>,
    receiver: CrossbeamReceiver<AccountWorkerJob>,
    storage_sender: CrossbeamSender<StorageWorkerJob>,
    availability: Arc<AvailabilitySheet>,
    cached_storage_roots: Arc<DashMap<B256, B256>>,
    stopped: Arc<AtomicBool>,
    worker_index: usize,
) where
    Factory: DatabaseProviderROFactory<Provider: TrieCursorFactory + HashedCursorFactory>
        + Clone
        + Send
        + Sync
        + 'static,
{
    availability.mark_idle(worker_index);
    while let Some(AccountWorkerJob::AccountMultiproof { input }) =
        next_job(&receiver, &stopped, &runtime).await
    {
        availability.mark_busy(worker_index);
        let start = runtime.now();
        #[cfg(feature = "trie-debug")]
        if let Some(delay) = task_ctx.proof_jitter {
            runtime.sleep(delay).await;
        }
        let AccountMultiproofInput { targets, proof_result_sender } = *input;
        let result = account_proof(
            &runtime,
            task_ctx.factory.clone(),
            &storage_sender,
            targets,
            Arc::clone(&cached_storage_roots),
        )
        .await;
        let ProofResultContext { sender, state, .. } = proof_result_sender;
        let elapsed = runtime.now().duration_since(start).unwrap_or_default();
        let _ = sender.send(ProofResultMessage { result, elapsed, state });
        availability.mark_idle(worker_index);
        runtime.yield_now().await;
    }
}

async fn account_proof<Factory>(
    runtime: &TaskRuntime,
    factory: Factory,
    storage_sender: &CrossbeamSender<StorageWorkerJob>,
    targets: MultiProofTargetsV2,
    cached_storage_roots: Arc<DashMap<B256, B256>>,
) -> Result<DecodedMultiProofV2, StateRootTaskError>
where
    Factory: DatabaseProviderROFactory<Provider: TrieCursorFactory + HashedCursorFactory>
        + Send
        + 'static,
{
    let MultiProofTargetsV2 { account_targets, storage_targets } = targets;
    let receivers = dispatch_v2_storage_proofs(storage_sender, &account_targets, storage_targets)?;
    let mut receivers: Vec<_> = receivers.into_iter().collect();
    receivers.sort_unstable_by_key(|(address, _)| *address);
    let mut completed = B256Map::default();
    for (address, receiver) in receivers {
        let result = loop {
            match receiver.try_recv() {
                Ok(result) => break result,
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    return Err(StateRootTaskError::ProofWorker(
                        "storage worker stopped before delivering its proof".into(),
                    ));
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    runtime.sleep(Duration::from_millis(1)).await;
                }
            }
        };
        // The production encoder has synchronous receive paths, including in Drop. Give it only
        // completed, single-result channels, so these paths can never block another actor.
        let (sender, receiver) = unbounded();
        let _ = sender.send(result);
        completed.insert(address, receiver);
    }
    runtime
        .spawn_cpu("account_proof", move || {
            let provider = factory.database_provider_ro()?;
            let mut calculator = proof_v2::ProofCalculator::new(
                provider.account_trie_cursor().map_err(ProviderError::from)?,
                provider.hashed_account_cursor().map_err(ProviderError::from)?,
            );
            let storage_calculator =
                Rc::new(RefCell::new(proof_v2::StorageProofCalculator::new_storage(
                    provider.storage_trie_cursor(B256::ZERO).map_err(ProviderError::from)?,
                    provider.hashed_storage_cursor(B256::ZERO).map_err(ProviderError::from)?,
                )));
            compute_account_proof(
                &mut calculator,
                storage_calculator,
                account_targets,
                completed,
                cached_storage_roots,
            )
            .map(|(proof, _)| proof)
        })
        .abort_on_drop()
        .await
        .map_err(worker_error)?
}

fn storage_proof<Factory>(
    factory: Factory,
    input: StorageProofInput,
) -> Result<StorageProofResult, StateProofError>
where
    Factory: DatabaseProviderROFactory<Provider: TrieCursorFactory + HashedCursorFactory>,
{
    let provider = factory
        .database_provider_ro()
        .map_err(|error| StateProofError::Database(DatabaseError::Other(error.to_string())))?;
    let proof_tx = ProofTaskTx::new(provider, 0);
    let mut calculator = proof_v2::StorageProofCalculator::new_storage(
        proof_tx.provider.storage_trie_cursor(B256::ZERO)?,
        proof_tx.provider.hashed_storage_cursor(B256::ZERO)?,
    );
    proof_tx.compute_v2_storage_proof(input, &mut calculator)
}

fn worker_error(error: TaskError) -> StateRootTaskError {
    StateRootTaskError::ProofWorker(error.to_string())
}
