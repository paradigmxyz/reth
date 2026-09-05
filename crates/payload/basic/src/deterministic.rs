//! Test the real job's deadlines, permit queue, cancellation, and detached fallback leases.

use super::*;
use commonware_runtime::{deterministic, Runner, Supervisor};
use reth_ethereum_engine_primitives::{EthBuiltPayload, EthPayloadAttributes};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};

#[derive(Clone, Debug, Default)]
struct Builder {
    events: Arc<Mutex<Vec<&'static str>>>,
    fallback: bool,
}

impl PayloadBuilder for Builder {
    type Attributes = EthPayloadAttributes;
    type BuiltPayload = EthBuiltPayload;

    fn try_build(
        &self,
        args: BuildArguments<Self::Attributes, Self::BuiltPayload>,
    ) -> Result<BuildOutcome<Self::BuiltPayload>, PayloadBuilderError> {
        assert!(args.cancel.is_cancelled(), "expired queued build must observe cancellation");
        self.events.lock().unwrap().push("canceled-build");
        Ok(BuildOutcome::Cancelled)
    }

    fn build_empty_payload(
        &self,
        _config: PayloadConfig<Self::Attributes, HeaderForPayload<Self::BuiltPayload>>,
    ) -> Result<Self::BuiltPayload, PayloadBuilderError> {
        self.events.lock().unwrap().push("empty");
        Err(PayloadBuilderError::MissingPayload)
    }

    fn on_missing_payload(
        &self,
        _args: BuildArguments<Self::Attributes, Self::BuiltPayload>,
    ) -> MissingPayloadBehaviour<Self::BuiltPayload> {
        if self.fallback {
            let events = self.events.clone();
            MissingPayloadBehaviour::RacePayload(Box::new(move || {
                events.lock().unwrap().push("fallback");
                Err(PayloadBuilderError::MissingPayload)
            }))
        } else {
            MissingPayloadBehaviour::RaceEmptyPayload
        }
    }
}

fn job(runtime: TaskRuntime, builder: Builder, duration: Duration) -> BasicPayloadJob<Builder> {
    BasicPayloadJob {
        config: PayloadConfig::new(
            Arc::new(SealedHeader::seal_slow(alloy_consensus::Header::default())),
            EthPayloadAttributes::default(),
            PayloadId::default(),
        ),
        executor: runtime.clone(),
        deadline: JobDeadline::new(runtime.clone(), duration, true),
        interval: JobInterval::new(runtime, Duration::from_millis(100), true),
        best_payload: PayloadState::Missing,
        pending_block: None,
        payload_task_guard: PayloadTaskGuard::new(1),
        cached_reads: None,
        execution_cache: None,
        state_root_handle: None,
        leases: Vec::new(),
        metrics: Default::default(),
        builder,
    }
}

#[derive(Debug)]
struct Lease(Arc<AtomicBool>);

impl Drop for Lease {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

async fn poll_once<F: Future + Unpin>(future: &mut F) -> Poll<F::Output> {
    // Commonware wakes the earliest sleeper before checking liveness. A noop waker would
    // falsely stall the runner if that timer fires before the scenario's next awaited sleep.
    futures_util::future::poll_fn(|cx| Poll::Ready(Pin::new(&mut *future).poll(cx))).await
}

fn simulate(seed: u64, native: Runtime) -> (String, Vec<&'static str>) {
    deterministic::Runner::new(
        deterministic::Config::default().with_seed(seed).with_timeout(Some(Duration::from_secs(5))),
    )
    .start(|context| async move {
        let runtime = TaskRuntime::deterministic(context.child("payload_jobs"));
        let builder = Builder::default();
        let generator = BasicPayloadJobGenerator::with_builder(
            (),
            native,
            BasicPayloadJobGeneratorConfig::default().deadline(Duration::from_secs(2)),
            builder.clone(),
        )
        .with_task_runtime(runtime.clone());
        let unix_now = runtime.now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        assert_eq!(generator.max_job_duration(unix_now), Duration::from_secs(2));
        assert_eq!(generator.max_job_duration(unix_now + 100), Duration::from_secs(8));

        // An unpolled deadline must remain anchored at construction, and a previously pending
        // timer must wake after its remaining virtual time rather than restarting its duration.
        let mut late = JobDeadline::new(runtime.clone(), Duration::from_millis(50), true);
        let mut active = JobDeadline::new(runtime.clone(), Duration::from_millis(50), true);
        assert!(poll_once(&mut active).await.is_pending());
        runtime.sleep(Duration::from_millis(5)).await;
        assert!(poll_once(&mut active).await.is_pending());
        runtime.sleep(Duration::from_millis(100)).await;
        assert!(poll_once(&mut late).await.is_ready());
        assert!(poll_once(&mut active).await.is_ready());

        // The first tick is immediate. A delayed build must retain the three missed Burst ticks.
        let mut interval = JobInterval::new(runtime.clone(), Duration::from_millis(100), true);
        assert!(poll_once(&mut futures_util::future::poll_fn(|cx| interval.poll_tick(cx)))
            .await
            .is_ready());
        runtime.sleep(Duration::from_millis(350)).await;
        for _ in 0..3 {
            assert!(poll_once(&mut futures_util::future::poll_fn(|cx| interval.poll_tick(cx)))
                .await
                .is_ready());
        }
        assert!(poll_once(&mut futures_util::future::poll_fn(|cx| interval.poll_tick(cx)))
            .await
            .is_pending());

        // Hold the only build permit while the actual job expires. Dropping it cancels the
        // detached attempt, which must see that cancellation once its permit becomes available.
        let mut expired = job(runtime.clone(), builder.clone(), Duration::from_millis(50));
        let permit = expired.payload_task_guard.acquire_owned().await;
        expired.spawn_build_job();
        let canceled = expired.pending_block.as_ref().unwrap().cancel.clone();
        runtime.sleep(Duration::from_millis(100)).await;
        assert!(matches!(poll_once(&mut expired).await, Poll::Ready(Ok(()))));
        drop(expired);
        assert!(canceled.is_cancelled());
        assert!(builder.events.lock().unwrap().is_empty());
        drop(permit);
        while builder.events.lock().unwrap().is_empty() {
            runtime.yield_now().await;
        }

        for fallback in [false, true] {
            let mut resolving = job(
                runtime.clone(),
                Builder { events: builder.events.clone(), fallback },
                Duration::from_secs(1),
            );
            // Keep a pending result without submitting another ordinary build, isolating the
            // fallback task's ownership of the lease after the payload job is removed.
            let (_sender, receiver) = oneshot::channel();
            resolving.pending_block = Some(PendingPayload::new(CancelOnDrop::default(), receiver));
            let released = Arc::new(AtomicBool::new(false));
            resolving.leases.push(PayloadBuilderLease::new(Lease(released.clone())));
            let (result, keep_alive) = resolving.resolve_kind(PayloadKind::Earliest);
            assert_eq!(keep_alive, KeepPayloadJobAlive::No);
            drop(resolving);
            assert!(!released.load(Ordering::SeqCst), "queued fallback must retain its lease");
            assert!(result.await.is_err());
            assert!(released.load(Ordering::SeqCst), "finished fallback must release its lease");
        }
        let events = builder.events.lock().unwrap().clone();
        assert_eq!(events, vec!["canceled-build", "empty", "fallback"]);
        (context.auditor().state(), events)
    })
}

#[test]
fn deterministic_payload_job_clock_and_cancellation() {
    let seeds: Vec<u64> = match std::env::var("RETH_DST_SEED") {
        Ok(seed) => vec![seed.parse().expect("RETH_DST_SEED must be a u64")],
        Err(std::env::VarError::NotPresent) => (0..16).collect(),
        Err(error) => panic!("invalid RETH_DST_SEED: {error}"),
    };
    let native = Runtime::test();
    for seed in seeds {
        eprintln!("payload job seed={seed}");
        assert_eq!(simulate(seed, native.clone()), simulate(seed, native.clone()));
    }
}
