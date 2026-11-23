//! Regression coverage for payload builder defaults taken from version metadata.

use clap::Parser;
use reth_node_core::{
    args::{DefaultPayloadBuilderValues, PayloadBuilderArgs},
    version::{try_init_version_metadata, RethCliVersionConsts},
};

#[derive(Parser)]
struct CommandParser {
    #[command(flatten)]
    builder: PayloadBuilderArgs,
}

#[test]
fn version_metadata_extra_data_applies_to_cli_and_default() {
    // Constructing builder defaults must not lock in metadata before the binary sets its own.
    drop(DefaultPayloadBuilderValues::default());
    try_init_version_metadata(RethCliVersionConsts {
        extra_data: "0xabcd".into(),
        ..Default::default()
    })
    .unwrap();

    let builder = CommandParser::parse_from(["reth"]).builder;
    assert_eq!(builder.extra_data.as_ref(), [0xab, 0xcd]);
    assert_eq!(PayloadBuilderArgs::default(), builder);
}
