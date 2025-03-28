use clap::Parser;

/// The arguments for the `reth bootnode` command.
/// see https://github.com/ethereum/go-ethereum/blob/14eb8967be7acc54c5dc9a416151ac45c01251b6/cmd/bootnode/main.go#L39-L48
/// for ref
#[derive(Parser, Debug)]
pub struct Command {
    /// Listen address for the bootnode (default: ":30301").
    #[arg(long, default_value = ":30301")]
    pub addr: String,

    /// Generate a new node key and save it to the specified file.
    #[arg(long, default_value = "")]
    pub gen_key: String,

    /// Write out the node's public key and quit.
    #[arg(long)]
    pub write_address: bool,

    /// Private key filename for the node.
    #[arg(long, default_value = "")]
    pub node_key: String,

    /// Private key as hex (for testing purposes).
    #[arg(long, default_value = "")]
    pub node_key_hex: String,

    /// NAT port mapping mechanism (e.g., "any", "none", "upnp", "pmp", "pmp:<IP>", "extip:<IP>").
    #[arg(long, default_value = "none")]
    pub nat: String,

    /// Restrict network communication to given IP networks (CIDR masks).
    #[arg(long, default_value = "")]
    pub net_restrict: String,

    /// Run a v5 topic discovery bootnode.
    #[arg(long)]
    pub v5: bool,
}

impl Command {
    pub fn execute(self) {
        println!("Bootnode started with config: {:?}", self);
        // Implement bootnode logic here
    }
}
