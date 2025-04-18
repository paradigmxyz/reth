//! Standalone bootnode command

use clap::Parser;
use reth_discv4::{DiscoveryUpdate, Discv4, Discv4Config};
use reth_discv5::{discv5::Event, Config, Discv5};
use reth_net_nat::NatResolver;
use reth_network_peers::NodeRecord;
use std::{net::SocketAddr, str::FromStr};
use tokio::select;
use tokio_stream::StreamExt;
use tracing::info;

/// Satrt a discovery only bootnode.
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
    /// Execute the bootnode command.
    pub async fn execute(self) -> eyre::Result<()> {
        info!("Bootnode started with config: {:?}", self);
        let sk = reth_network::config::rng_secret_key();
        let socket_addr = SocketAddr::from_str(&self.addr)?;
        let local_enr = NodeRecord::from_secret_key(socket_addr, &sk);

        let config = Discv4Config::builder().external_ip_resolver(Some(self.nat)).build();

        let (_discv4, mut discv4_service) =
            Discv4::bind(socket_addr, local_enr, sk, config).await?;

        info!("Started discv4 at address:{:?}", socket_addr);

        let mut discv4_updates = discv4_service.update_stream();
        discv4_service.spawn();

        // Optional discv5 update event listener if v5 is enabled
        let mut discv5_updates = None;

        if self.v5 {
            info!("Starting discv5");
            let config = Config::builder(socket_addr).build();
            let (_discv5, updates, _local_enr_discv5) = Discv5::start(&sk, config).await?;
            discv5_updates = Some(updates);
        };

        // event info loop for logging
        loop {
            select! {
                //discv4 updates
                update = discv4_updates.next() => {
                    if let Some(update) = update {
                        match update {
                            DiscoveryUpdate::Added(record) => {
                                info!("(Discv4) new peer added, peer_id={:?}", record.id);
                            }
                            DiscoveryUpdate::Removed(peer_id) => {
                                info!("(Discv4) peer with peer-id={:?} removed", peer_id);
                            }
                            _ => {}
                        }
                    } else {
                        info!("(Discv4) update stream ended.");
                        break;
                    }
                }
                //if discv5, discv5 update stream, else do nothing
                update = async {
                    if let Some(updates) = &mut discv5_updates {
                        updates.recv().await
                    } else {
                        futures::future::pending().await
                    }
                } => {
                    if let Some(update) = update {
                     if let Event::SessionEstablished(enr, _) = update {
                            info!("(Discv5) new peer added, peer_id={:?}", enr.id());
                        }
                    } else {
                        info!("(Discv5) update stream ended.");
                        break;
                    }
                }
            }
        }

        Ok(())
    }
}
