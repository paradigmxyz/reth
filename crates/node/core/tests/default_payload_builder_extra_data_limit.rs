//! Regression coverage for payload builder defaults above the CLI extra data limit.

use alloy_consensus::constants::MAXIMUM_EXTRA_DATA_SIZE;
use clap::{error::ErrorKind, Parser};
use reth_node_core::{
    args::{DefaultPayloadBuilderValues, PayloadBuilderArgs},
    node_config::NodeConfig,
};
use std::sync::Arc;

#[derive(Debug, Parser)]
struct CommandParser {
    #[command(flatten)]
    builder: PayloadBuilderArgs,
}

#[test]
fn oversized_default_is_only_limited_by_cli() {
    let extra_data = format!("0x{}", "ab".repeat(MAXIMUM_EXTRA_DATA_SIZE + 1));
    DefaultPayloadBuilderValues::default().with_extra_data(extra_data).try_init().unwrap();

    let builder = NodeConfig::new(Arc::new(())).builder;
    assert_eq!(builder.extra_data.as_ref(), vec![0xab; MAXIMUM_EXTRA_DATA_SIZE + 1].as_slice());

    let err = CommandParser::try_parse_from(["reth"]).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidValue);
}
