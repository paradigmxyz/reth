//! Integration tests for customizable dev CLI defaults.

use std::{num::NonZeroUsize, time::Duration};

use clap::{Args, Parser};
use reth_node_core::args::{DefaultDevArgs, DevArgs};

#[derive(Parser)]
struct CommandParser<T: Args> {
    #[command(flatten)]
    args: T,
}

#[test]
fn custom_dev_defaults_apply_to_cli_and_default() {
    let defaults = DefaultDevArgs::default()
        .with_dev(true)
        .with_block_max_transactions(Some(3))
        .with_finality_depth(NonZeroUsize::new(2).unwrap())
        .with_payload_wait_time(Some(Duration::from_millis(250)))
        .with_dev_mnemonic("custom mnemonic".to_string());
    defaults.try_init().unwrap();

    let expected = DevArgs {
        dev: true,
        block_max_transactions: Some(3),
        block_time: None,
        finality_depth: NonZeroUsize::new(2).unwrap(),
        payload_wait_time: Some(Duration::from_millis(250)),
        dev_mnemonic: "custom mnemonic".to_string(),
    };

    assert_eq!(CommandParser::<DevArgs>::parse_from(["reth"]).args, expected);
    assert_eq!(DevArgs::default(), expected);
}
