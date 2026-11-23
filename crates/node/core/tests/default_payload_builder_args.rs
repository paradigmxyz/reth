//! Regression coverage for customized payload builder defaults.

use alloy_primitives::{b256, hex::FromHexError};
use clap::Parser;
use reth_node_core::{
    args::{DefaultPayloadBuilderValues, PayloadBuilderArgs, PayloadBuilderDefaultsError},
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
    let defaults = DefaultPayloadBuilderValues::default()
        .with_max_payload_tasks(7)
        .with_extra_data("0xgg")
        .try_init()
        .unwrap_err();
    let err = defaults.with_extra_data("0x0xab").try_init_detailed().unwrap_err();
    assert_eq!(err.to_string(), "invalid hex in payload builder default extra data");
    let PayloadBuilderDefaultsError::InvalidExtraData { defaults, source } = err else {
        panic!("expected invalid extra data, got {err}");
    };
    assert_eq!(source, FromHexError::InvalidHexCharacter { c: 'x', index: 1 });

    let expected = b256!("000102030405060708090a0b0c0d0e0ff0f1f2f3f4f5f6f7f8f9fafbfcfdfeff");
    defaults.with_extra_data(expected.to_string()).try_init().unwrap();

    let err = DefaultPayloadBuilderValues::default()
        .with_extra_data("0xgg")
        .try_init_detailed()
        .unwrap_err();
    assert!(matches!(err, PayloadBuilderDefaultsError::AlreadyInitialized(_)));

    let builder = CommandParser::parse_from(["reth"]).builder;
    assert_eq!(builder.extra_data.as_ref(), expected.as_slice());
    assert_eq!(builder.max_payload_tasks, 7);
    assert_eq!(NodeConfig::new(Arc::new(())).builder, builder);

    let args = CommandParser::parse_from(["reth", "--builder.extradata", "café"]);
    assert_eq!(args.builder.extra_data.as_ref(), "café".as_bytes());
}
