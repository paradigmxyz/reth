use clap::Parser;
use rand::thread_rng;
use reth_discv4::{Discv4, Discv4Config};
use reth_discv5::{Config, Discv5};
use reth_net_nat::NatResolver;
use reth_network_peers::NodeRecord;
use secp256k1::SECP256K1;
use std::{net::SocketAddr, str::FromStr};
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
    pub async fn execute(self) -> eyre::Result<()> {
        // Implement bootnode logic here
        println!("Bootnode started with config: {:?}", self);
        // generate a (random) keypair
        let mut rng = thread_rng();
        let (sk, pk) = SECP256K1.generate_keypair(&mut rng);

        let socket_addr = SocketAddr::from_str(&self.addr)?;
        let local_enr = NodeRecord::from_secret_key(socket_addr, &sk);

        let discv4_future = async {
            let config = Discv4Config::builder().external_ip_resolver(Some(self.nat)).build();

            let (discv4, mut discv4_service) =
                Discv4::bind(socket_addr, local_enr, sk, config).await.unwrap();

            let discv4_updates = discv4_service.update_stream();
            let discv4_service = discv4_service.spawn();

            Ok((Some(discv4), Some(discv4_updates), Some(discv4_service)))
        };

        let discv5_future = async {
            if self.v5 == false {
                return Ok((None, None));
            }
            let config = Config::builder(socket_addr).build();
            let (discv5, discv5_updates, _local_enr_discv5) = Discv5::start(&sk, config).await.unwrap();
            Ok((Some(discv5), Some(discv5_updates.into())))
        };

        let _ = tokio::try_join!(discv4_future, discv5_future).unwrap();
        Ok(())
    }
}
