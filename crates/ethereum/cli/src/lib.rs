//! Reth CLI implementation.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

/// Chain specification parser.
pub mod chainspec;

#[cfg(test)]
mod test {
    use clap::Parser;
    use reth_chainspec::DEV;
    use reth_cli_commands::NodeCommand;

    #[test]
    #[ignore = "reth cmd will print op-reth output if optimism feature enabled"]
    fn parse_dev() {
        let cmd: NodeCommand = NodeCommand::parse_from(["reth", "--dev"]);
        let chain = DEV.clone();
        assert_eq!(cmd.chain.chain, chain.chain);
        assert_eq!(cmd.chain.genesis_hash, chain.genesis_hash);
        assert_eq!(
            cmd.chain.paris_block_and_final_difficulty,
            chain.paris_block_and_final_difficulty
        );
        assert_eq!(cmd.chain.hardforks, chain.hardforks);

        assert!(cmd.rpc.http);
        assert!(cmd.network.discovery.disable_discovery);

        assert!(cmd.dev.dev);
    }
}
