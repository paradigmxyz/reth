//! Diagnostic phase accounting. CPU and context-switch counters belong only to the calling thread.
use reth_metrics::thread::{
    current_thread_cpu_time, current_thread_runqueue_time, ThreadResourceUsage,
};
use std::{
    cell::Cell,
    sync::atomic::{AtomicU64, Ordering},
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

    /// Measures a fine-grained phase only when detailed activity is explicitly enabled.
    pub fn detail(phase: &'static str) -> Self {
        if !tracing::enabled!(target: "engine::tree::detail_activity", tracing::Level::TRACE) {
            return Self { active: None };
        }
        Self::new(phase)
    }

    /// Measures one worker job and records its relation to the submitting batch.
    pub fn job(phase: &'static str, parent: u64, units: usize, queued: Duration) -> Self {
        if !tracing::enabled!(target: "engine::tree::detail_activity", tracing::Level::TRACE) {
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
        if !tracing::enabled!(target: "engine::tree::activity", tracing::Level::TRACE) {
            return Self { active: None };
        }
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let span = tracing::trace_span!(target: "engine::tree::activity", "trie_activity",
            phase, id, parent_id = parent, units, queued_us = queued.as_secs_f64() * 1e6,
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
        active.span.record("wall_us", active.start.elapsed().as_secs_f64() * 1e6);
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

#[derive(Debug)]
struct Active {
    id: u64,
    previous: u64,
    span: EnteredSpan,
    start: Instant,
    usage: ThreadResourceUsage,
    cpu: Option<Duration>,
    queue: Option<Duration>,
}
static NEXT: AtomicU64 = AtomicU64::new(1);
thread_local! { static PARENT: Cell<u64> = const { Cell::new(0) }; }
