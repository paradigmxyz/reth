//! Opt-in buffered diagnostic timelines. Not intended for production telemetry.
use reth_metrics::thread::{current_thread_cpu_time, current_thread_runqueue_time};
use std::{
    cell::{Cell, RefCell},
    marker::PhantomData,
    rc::Rc,
    sync::{
        atomic::{AtomicU64, Ordering},
        LazyLock,
    },
    time::{Duration, Instant},
};
use tracing::span::EnteredSpan;

/// A thread-bound phase. Nested records are buffered until the outer scope ends.
#[derive(Debug)]
pub struct ActivityGuard {
    active: Option<Active>,
    _thread: PhantomData<Rc<()>>,
}

impl ActivityGuard {
    /// Records an outer or nested phase when diagnostic tracing is enabled.
    pub fn new(phase: &'static str) -> Self {
        Self::start(phase, 0, 0, Duration::ZERO)
    }
    /// Enables coordinator phase detail independently of engine phases.
    pub fn coordinator(phase: &'static str) -> Self {
        if tracing::enabled!(target: "engine::tree::coordinator_activity", tracing::Level::TRACE) {
            Self::new(phase)
        } else {
            Self::disabled()
        }
    }
    /// Records fine-grained phases without per-event subscriber calls.
    pub fn detail(phase: &'static str) -> Self {
        if tracing::enabled!(target: "engine::tree::detail_activity", tracing::Level::TRACE) {
            Self::new(phase)
        } else {
            Self::disabled()
        }
    }
    /// Records a worker scope and the dispatch-to-start interval.
    pub fn worker(phase: &'static str, parent: u64, units: usize, queued: Duration) -> Self {
        Self::start(phase, parent, units, queued)
    }
    /// Records a storage job only with fine-grained tracing enabled.
    pub fn job(phase: &'static str, parent: u64, units: usize, queued: Duration) -> Self {
        if tracing::enabled!(target: "engine::tree::detail_activity", tracing::Level::TRACE) {
            Self::start(phase, parent, units, queued)
        } else {
            Self::disabled()
        }
    }
    const fn disabled() -> Self {
        Self { active: None, _thread: PhantomData }
    }
    fn start(phase: &'static str, external_parent: u64, units: usize, queued: Duration) -> Self {
        if !tracing::enabled!(target: "engine::tree::activity", tracing::Level::TRACE) {
            return Self::disabled()
        }
        let root = PARENT.with(Cell::get) == 0;
        let allowed_root =
            matches!(phase, "task" | "new_payload" | "bal_worker" | "hashing_stream");
        #[cfg(test)]
        let allowed_root = allowed_root || phase.starts_with("test_");
        // Do not turn each small Rayon job into an outer trace/export by default.
        // Nested work executed on the coordinator itself is still accounted for.
        if root &&
            !allowed_root &&
            !tracing::enabled!(target: "engine::tree::worker_activity", tracing::Level::TRACE)
        {
            return Self::disabled()
        }
        let epoch = *EPOCH;
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let previous = PARENT.with(|p| p.replace(id));
        let outer = previous == 0;
        let span = outer.then(|| tracing::trace_span!(target: "engine::tree::activity", "buffered_activity", phase, id).entered());
        // /proc scheduler reads are restricted to outer scopes. Optional phase CPU clocks
        // are benchmarked separately from wall-only probes to expose measurement overhead.
        let queue = outer.then(current_thread_runqueue_time).flatten();
        let cpu_enabled =
            outer || tracing::enabled!(target: "engine::tree::phase_cpu", tracing::Level::TRACE);
        let cpu = cpu_enabled.then(current_thread_cpu_time).flatten();
        let start = Instant::now().duration_since(epoch).as_nanos() as u64;
        Self {
            active: Some(Active {
                id,
                previous,
                phase,
                parent: if outer { external_parent } else { previous },
                units,
                queued: queued.as_nanos() as u64,
                start,
                cpu,
                queue,
                span,
                dependencies: Cell::new([0; 4]),
                before: Cell::new([0; 3]),
                after: Cell::new([0; 3]),
                wake: Cell::new(""),
            }),
            _thread: PhantomData,
        }
    }
    /// Outstanding dependencies at phase entry.
    pub fn dependencies(&self, storage: usize, proofs: usize, accounts: usize, draining: bool) {
        if let Some(a) = &self.active {
            a.dependencies.set([storage, proofs, accounts, usize::from(draining)]);
        }
    }
    /// Queue lengths before and after a synchronous coordinator operation.
    pub fn queues(&self, before: bool, updates: usize, proofs: usize, storage: usize) {
        if let Some(a) = &self.active {
            if before {
                a.before.set([updates, proofs, storage]);
            } else {
                a.after.set([updates, proofs, storage]);
            }
        }
    }
    /// Channel that ended a receive.
    pub fn wake(&self, kind: &'static str) {
        if let Some(a) = &self.active {
            a.wake.set(kind);
        }
    }
    /// Cross-thread correlation identifier.
    pub fn id(&self) -> u64 {
        self.active.as_ref().map_or(0, |a| a.id)
    }
}

impl Drop for ActivityGuard {
    fn drop(&mut self) {
        let Some(a) = self.active.take() else { return };
        let end = EPOCH.elapsed().as_nanos() as u64;
        let cpu_end = a.cpu.and_then(|_| current_thread_cpu_time());
        let queue_end = a.queue.and_then(|_| current_thread_runqueue_time());
        PARENT.with(|p| p.set(a.previous));
        EVENTS.with(|events| {
            let mut events = events.borrow_mut();
            // Bound diagnostics even on adversarial workloads; omissions are explicit.
            if events.records.len() < 262144 {
                events.records.push(Record { id:a.id, parent:a.parent, phase:a.phase, start:a.start, end, cpu_start:a.cpu.map(|v| v.as_nanos() as u64), cpu_end:cpu_end.map(|v| v.as_nanos() as u64), queued:a.queued, units:a.units, dependencies:a.dependencies.get(), before:a.before.get(), after:a.after.get(), wake:a.wake.get() });
            } else { events.dropped += 1; }
            if a.previous == 0 {
                let batch = Batch {
                    id: a.id, phase: a.phase,
                    source_thread: std::thread::current().name().unwrap_or("unnamed").to_owned(),
                    source_id: format!("{:?}", std::thread::current().id()),
                    records: std::mem::take(&mut events.records),
                    dropped: std::mem::take(&mut events.dropped),
                    runqueue_ns: a.queue.zip(queue_end).map(|(s,e)| e.saturating_sub(s).as_nanos() as u64),
                };
                if WRITER.try_send(batch).is_err() {
                    tracing::warn!(target: "engine::tree::activity", "diagnostic writer queue full: timeline incomplete");
                }
            }
        });
        drop(a.span);
    }
}
#[derive(Debug)]
struct Active {
    id: u64,
    previous: u64,
    parent: u64,
    phase: &'static str,
    start: u64,
    cpu: Option<Duration>,
    queue: Option<Duration>,
    units: usize,
    queued: u64,
    span: Option<EnteredSpan>,
    dependencies: Cell<[usize; 4]>,
    before: Cell<[usize; 3]>,
    after: Cell<[usize; 3]>,
    wake: Cell<&'static str>,
}
#[derive(Debug)]
#[allow(dead_code)] // Fields are serialized by Debug in the diagnostic trace.
struct Record {
    id: u64,
    parent: u64,
    phase: &'static str,
    start: u64,
    end: u64,
    cpu_start: Option<u64>,
    cpu_end: Option<u64>,
    queued: u64,
    units: usize,
    dependencies: [usize; 4],
    before: [usize; 3],
    after: [usize; 3],
    wake: &'static str,
}
#[derive(Default)]
struct Events {
    records: Vec<Record>,
    dropped: usize,
}
static EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);
static NEXT: AtomicU64 = AtomicU64::new(1);
thread_local! {
    static PARENT: Cell<u64> = const { Cell::new(0) };
    static EVENTS: RefCell<Events> = RefCell::new(Events::default());
}

struct Batch {
    id: u64,
    phase: &'static str,
    source_thread: String,
    source_id: String,
    records: Vec<Record>,
    dropped: usize,
    runqueue_ns: Option<u64>,
}
static WRITER: LazyLock<std::sync::mpsc::SyncSender<Batch>> = LazyLock::new(|| {
    let (tx, rx) = std::sync::mpsc::sync_channel::<Batch>(256);
    let dispatch = tracing::dispatcher::get_default(Clone::clone);
    std::thread::Builder::new().name("trie-diagnostic-writer".into()).spawn(move || {
        tracing::dispatcher::with_default(&dispatch, || {
            while let Ok(batch) = rx.recv() {
                tracing::trace!(target: "engine::tree::activity", id=batch.id, phase=batch.phase,
                    source_thread=batch.source_thread, source_id=batch.source_id,
                    timeline=?batch.records, dropped=batch.dropped, runqueue_ns=?batch.runqueue_ns,
                    "buffered timeline");
            }
        });
    }).expect("diagnostic writer thread");
    tx
});

#[cfg(test)]
mod tests {
    use super::*;
    use reth_tracing::tracing_subscriber::{layer::SubscriberExt, Layer, Registry};
    use std::sync::mpsc;
    struct Capture(mpsc::Sender<String>);
    impl<S: tracing::Subscriber> Layer<S> for Capture {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _: reth_tracing::tracing_subscriber::layer::Context<'_, S>,
        ) {
            struct Visitor<'a>(&'a mpsc::Sender<String>);
            impl tracing::field::Visit for Visitor<'_> {
                fn record_debug(
                    &mut self,
                    field: &tracing::field::Field,
                    value: &dyn std::fmt::Debug,
                ) {
                    if field.name() == "timeline" {
                        let _ = self.0.send(format!("{value:?}"));
                    }
                }
            }
            event.record(&mut Visitor(&self.0));
        }
    }
    #[test]
    fn nested_phases_export_from_writer_with_original_parent() {
        let (tx, rx) = mpsc::channel();
        tracing::subscriber::with_default(Registry::default().with(Capture(tx)), || {
            let outer = ActivityGuard::new("test_outer");
            let id = outer.id();
            {
                let child = ActivityGuard::detail("test_child");
                child.queues(true, 1, 2, 3);
                child.wake("proof");
            }
            EVENTS.with(|e| {
                let e = e.borrow();
                assert_eq!(e.records.len(), 1);
                assert_eq!(e.records[0].parent, id);
                assert!(e.records[0].end >= e.records[0].start);
            });
            drop(outer);
            assert_eq!(PARENT.with(Cell::get), 0);
            let exported =
                rx.recv_timeout(Duration::from_secs(5)).expect("writer exports buffered records");
            assert!(exported.contains("test_child"));
            assert!(exported.contains("test_outer"));
            assert!(exported.contains("before: [1, 2, 3]"));
        });
    }
}
