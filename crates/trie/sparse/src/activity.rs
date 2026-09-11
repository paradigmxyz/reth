//! Diagnostic phase accounting. CPU and context-switch counters belong only to the calling thread.
use reth_metrics::thread::{
    current_thread_cpu_time, current_thread_runqueue_time, ThreadResourceUsage,
};
use std::{
    cell::Cell,
    sync::{
        atomic::{AtomicU64, Ordering},
        LazyLock,
    },
    time::{Duration, Instant},
};
use tracing::span::EnteredSpan;

/// Thread-bound measurement of a synchronous trie phase.
#[derive(Debug)]
pub struct ActivityGuard {
    active: Option<Active>,
}

impl ActivityGuard {
    /// Measures a phase, including scheduler counters when available.
    pub fn new(phase: &'static str) -> Self {
        Self::start(phase, PARENT.with(Cell::get), 0, Duration::ZERO, true)
    }

    /// Measures a phase and records how much work it was handed.
    pub fn sized(phase: &'static str, units: usize) -> Self {
        Self::start(phase, PARENT.with(Cell::get), units, Duration::ZERO, true)
    }

    /// Measures a phase that runs off the thread that submitted it, so its parent cannot be
    /// taken from the thread-local stack.
    pub fn linked(phase: &'static str, parent: u64, units: usize, queued: Duration) -> Self {
        Self::start(phase, parent, units, queued, false)
    }

    /// Measures a fine-grained phase only when detailed activity is explicitly enabled.
    pub fn detail(phase: &'static str) -> Self {
        if !detail_enabled() {
            return Self { active: None };
        }
        Self::new(phase)
    }

    /// Measures one worker job and records its relation to the submitting batch.
    pub fn job(phase: &'static str, parent: u64, units: usize, queued: Duration) -> Self {
        if !detail_enabled() {
            return Self { active: None };
        }
        Self::start(phase, parent, units, queued, false)
    }

    fn start(
        phase: &'static str,
        parent: u64,
        units: usize,
        queued: Duration,
        scheduler: bool,
    ) -> Self {
        if !enabled() {
            return Self { active: None };
        }
        let epoch = *EPOCH;
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let span = tracing::trace_span!(target: "engine::tree::activity", "trie_activity",
            phase, id, parent_id = parent, units, queued_us = queued.as_secs_f64() * 1e6,
            wall_start_ns = tracing::field::Empty, wall_end_ns = tracing::field::Empty,
            cpu_start_ns = tracing::field::Empty, cpu_end_ns = tracing::field::Empty,
            wall_us = tracing::field::Empty, cpu_us = tracing::field::Empty,
            queue_start_ns = tracing::field::Empty, queue_end_ns = tracing::field::Empty,
            runqueue_us = tracing::field::Empty, voluntary = tracing::field::Empty,
            involuntary = tracing::field::Empty, minor_faults = tracing::field::Empty,
            major_faults = tracing::field::Empty, input_ops = tracing::field::Empty,
        )
        .entered();
        let previous = PARENT.with(|p| p.replace(id));
        Self {
            active: Some(Active {
                id,
                previous,
                span,
                start: Instant::now(),
                epoch,
                cpu: current_thread_cpu_time(),
                usage: ThreadResourceUsage::now(),
                queue: scheduler.then(current_thread_runqueue_time).flatten(),
            }),
        }
    }

    /// Identifier for attaching worker jobs to this batch.
    pub fn id(&self) -> u64 {
        self.active.as_ref().map_or(0, |a| a.id)
    }
}

impl Drop for ActivityGuard {
    fn drop(&mut self) {
        let Some(active) = &self.active else { return };
        let queue = active.queue.and_then(|_| current_thread_runqueue_time());
        let usage = active.usage.elapsed();
        let cpu = current_thread_cpu_time();
        let end = Instant::now();
        active.span.record("wall_us", end.duration_since(active.start).as_secs_f64() * 1e6);
        active
            .span
            .record("wall_start_ns", active.start.duration_since(active.epoch).as_nanos() as u64);
        active.span.record("wall_end_ns", end.duration_since(active.epoch).as_nanos() as u64);
        if let (Some(start), Some(end)) = (active.cpu, cpu) {
            active.span.record("cpu_start_ns", start.as_nanos() as u64);
            active.span.record("cpu_end_ns", end.as_nanos() as u64);
            active.span.record("cpu_us", end.saturating_sub(start).as_secs_f64() * 1e6);
        }
        if let (Some(start), Some(end)) = (active.queue, queue) {
            active.span.record("queue_start_ns", start.as_nanos() as u64);
            active.span.record("queue_end_ns", end.as_nanos() as u64);
            active.span.record("runqueue_us", end.saturating_sub(start).as_secs_f64() * 1e6);
        }
        if let Some(usage) = usage {
            active.span.record("voluntary", usage.voluntary_context_switches);
            active.span.record("involuntary", usage.involuntary_context_switches);
            active.span.record("minor_faults", usage.minor_page_faults);
            active.span.record("major_faults", usage.major_page_faults);
            active.span.record("input_ops", usage.block_input_operations);
        }
        PARENT.with(|p| p.set(active.previous));
    }
}

/// Running total of a phase that runs far too often for a record each.
///
/// A phase invoked once per streamed state update cannot afford an [`ActivityGuard`]: its
/// scheduler counters are two syscalls and its span carries a dozen fields. This only reads the
/// monotonic clock, so the CPU share of a tallied phase is not separated out and stays in the
/// residual of its enclosing guard.
#[derive(Clone, Copy, Debug, Default)]
pub struct WallTally {
    /// Times the phase ran.
    pub count: u64,
    /// Wall time the phase took.
    pub wall: Duration,
}

impl WallTally {
    /// Starts one measurement, when diagnostics are enabled.
    pub fn start() -> Option<Instant> {
        enabled().then(Instant::now)
    }

    /// Adds one measurement started with [`Self::start`].
    pub fn add(&mut self, start: Option<Instant>) {
        let Some(start) = start else { return };
        self.count += 1;
        self.wall += start.elapsed();
    }
}

/// Whether diagnostic activity accounting is enabled.
pub fn enabled() -> bool {
    tracing::enabled!(target: "engine::tree::activity", tracing::Level::TRACE)
}

/// Whether per-job activity accounting is enabled on top of [`enabled`].
pub fn detail_enabled() -> bool {
    tracing::enabled!(target: "engine::tree::detail_activity", tracing::Level::TRACE)
}

/// Nanoseconds since the process-wide diagnostic epoch that every activity record is stamped
/// against, so timelines taken on different threads can be laid over each other.
pub fn epoch_ns() -> u64 {
    ns_since_epoch(Instant::now())
}

/// Stamps an instant that was already read against the diagnostic epoch, for a hot path that
/// cannot afford a second clock read.
pub fn ns_since_epoch(at: Instant) -> u64 {
    at.saturating_duration_since(*EPOCH).as_nanos() as u64
}

#[derive(Debug)]
struct Active {
    id: u64,
    previous: u64,
    span: EnteredSpan,
    start: Instant,
    epoch: Instant,
    usage: ThreadResourceUsage,
    cpu: Option<Duration>,
    queue: Option<Duration>,
}
static EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);
static NEXT: AtomicU64 = AtomicU64::new(1);
thread_local! { static PARENT: Cell<u64> = const { Cell::new(0) }; }
