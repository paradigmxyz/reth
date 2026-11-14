//! Reports which executor ran each block, as one JSON object per line on stderr.

use reth_engine_tree::tree::payload_validator::BAL_EXECUTION_PATH_TARGET;
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
    Layer,
};

/// Installs the global subscriber that prints the engine's execution path events.
pub(crate) fn init() {
    tracing_subscriber::registry()
        .with(
            ExecutionPathLayer
                .with_filter(Targets::new().with_target(BAL_EXECUTION_PATH_TARGET, Level::DEBUG)),
        )
        .init();
}

/// Prints each event on [`BAL_EXECUTION_PATH_TARGET`] as a `balExecution` line.
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
