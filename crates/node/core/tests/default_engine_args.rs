//! Regression coverage for configurable engine CLI defaults.

use clap::Parser;
use reth_node_core::args::{DefaultEngineValues, EngineArgs};
use std::time::Duration;

#[derive(Parser)]
struct CommandParser {
    #[command(flatten)]
    args: EngineArgs,
}

#[test]
fn disabled_state_root_timeout_default_applies_to_cli() {
    DefaultEngineValues::default().with_state_root_task_timeout(None).try_init().unwrap();

    assert_eq!(CommandParser::parse_from(["reth"]).args, EngineArgs::default());
    assert_eq!(
        CommandParser::parse_from(["reth", "--engine.state-root-task-timeout", "500ms"])
            .args
            .state_root_task_timeout,
        Some(Duration::from_millis(500))
    );
}
