use clap::Parser;
use reth_net_nat::NatResolver;
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

    /// Private key filename for the node.
    #[arg(long, default_value = "")]
    pub node_key: String,

    /// NAT resolution method (any|none|upnp|publicip|extip:\<IP\>)
    #[arg(long, default_value = "any")]
    pub nat: NatResolver,

    /// Run a v5 topic discovery bootnode.
    #[arg(long)]
    pub v5: bool,
}

impl Command {
    pub async fn execute(self) {
        println!("Bootnode started with config: {:?}", self);
        // Implement bootnode logic here
    }
}
