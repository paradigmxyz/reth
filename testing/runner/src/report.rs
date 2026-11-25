//! Writes client logs and optional per-block execution path reports to stderr.

use serde::Serialize;
use std::{fmt, io::Write};
use tracing::{
    field::{Field, Visit},
    Event, Level, Subscriber,
};
use tracing_subscriber::{
    filter::Targets,
    layer::{Context, SubscriberExt},
    util::SubscriberInitExt,
    EnvFilter, Layer,
};

/// Enables client logs via `RUST_LOG` and filters the structured BAL report independently.
pub(crate) fn init(bal_report: bool) {
    let client = std::env::var_os(EnvFilter::DEFAULT_ENV).filter(|v| !v.is_empty()).map(|value| {
        tracing_subscriber::fmt::layer()
            .with_writer(std::io::stderr)
            .with_ansi(false)
            .with_filter(EnvFilter::new(value.to_string_lossy()))
    });
    if client.is_none() && !bal_report {
        return
    }
    let bal = bal_report.then(|| {
        ExecutionPathLayer.with_filter(
            Targets::new().with_target("engine::tree::bal_execution_path", Level::DEBUG),
        )
    });
    tracing_subscriber::registry().with(client).with(bal).init();
}

/// Prints each execution-path event as a `balExecution` line.
struct ExecutionPathLayer;

impl<S: Subscriber> Layer<S> for ExecutionPathLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut fields = ExecutionPathFields { event: "balExecution", ..Default::default() };
        event.record(&mut fields);
        let line = serde_json::to_string(&fields).expect("fields serialize");
        // One write per line, so lines from concurrent fixtures do not interleave.
        let _ = writeln!(std::io::stderr().lock(), "{line}");
    }
}

/// One `balExecution` line. Fields serialize in declaration order.
#[derive(Default, Serialize)]
struct ExecutionPathFields {
    event: &'static str,
    block: u64,
    hash: String,
    path: String,
    reason: String,
}

impl Visit for ExecutionPathFields {
    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name() == "block" {
            self.block = value;
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "path" => self.path = value.to_string(),
            "reason" => self.reason = value.to_string(),
            _ => {}
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "hash" {
            self.hash = format!("{value:?}");
        }
    }
}
