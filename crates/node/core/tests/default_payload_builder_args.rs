//! Regression coverage for customized payload builder defaults.

use alloy_consensus::constants::MAXIMUM_EXTRA_DATA_SIZE;
use alloy_primitives::b256;
use clap::Parser;
use reth_node_core::{
    args::{DefaultPayloadBuilderValues, PayloadBuilderArgs},
    node_config::NodeConfig,
};
use std::sync::Arc;

#[derive(Parser)]
struct CommandParser {
    #[command(flatten)]
    builder: PayloadBuilderArgs,
}

#[test]
fn custom_extra_data_applies_to_cli_and_node_config() {
    // A rejected default must leave the global slot available for a valid configuration.
    for invalid in [
        "0xgg".to_string(),
        "0xabc".to_string(),
        "0x0xab".to_string(),
        format!("0x{}", "ab".repeat(MAXIMUM_EXTRA_DATA_SIZE + 1)),
        format!("{}x", "é".repeat(MAXIMUM_EXTRA_DATA_SIZE / 2)),
    ] {
        DefaultPayloadBuilderValues::default().with_extra_data(invalid).try_init().unwrap_err();
    }

    let expected = b256!("000102030405060708090a0b0c0d0e0ff0f1f2f3f4f5f6f7f8f9fafbfcfdfeff");
    DefaultPayloadBuilderValues::default()
        .with_extra_data(expected.to_string())
        .try_init()
        .unwrap();

    assert_eq!(
        CommandParser::parse_from(["reth"]).builder.extra_data.as_ref(),
        expected.as_slice()
    );
    assert_eq!(NodeConfig::new(Arc::new(())).builder.extra_data.as_ref(), expected.as_slice());

    let args = CommandParser::parse_from(["reth", "--builder.extradata", "café"]);
    assert_eq!(args.builder.extra_data.as_ref(), "café".as_bytes());
}
