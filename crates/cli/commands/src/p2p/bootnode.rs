//! Standalone bootnode command

use clap::Parser;
use reth_cli_util::{get_secret_key, load_secret_key::rng_secret_key};
use reth_discv4::{DiscoveryUpdate, Discv4, Discv4Config};
use reth_discv5::{discv5::Event, Config, Discv5};
use reth_net_nat::NatResolver;
use reth_network_peers::NodeRecord;
use secp256k1::SecretKey;
use std::{net::SocketAddr, path::PathBuf};
use tokio::select;
use tokio_stream::StreamExt;
use tracing::info;

/// Start a discovery only bootnode.
#[derive(Parser, Debug)]
pub struct Command {
    /// Listen address for the bootnode (default: "0.0.0.0:30301").
    #[arg(long, default_value = "0.0.0.0:30301")]
    pub addr: SocketAddr,

    /// Secret key to use for the bootnode.
    ///
    /// This will also deterministically set the peer ID.  
    /// If a path is provided but no key exists at that path,  
    /// a new random secret will be generated and stored there.  
    /// If no path is specified, a new ephemeral random secret will be used.
    #[arg(long, value_name = "PATH")]
    pub p2p_secret_key: Option<PathBuf>,

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
        info!("Bootnode started with config: {self:?}");

        let sk = self.network_secret()?;
        let local_enr = NodeRecord::from_secret_key(self.addr, &sk);

        let config = Discv4Config::builder().external_ip_resolver(Some(self.nat)).build();

        let (_discv4, mut discv4_service) = Discv4::bind(self.addr, local_enr, sk, config).await?;

        info!("Started discv4 at address: {local_enr:?}");

        let mut discv4_updates = discv4_service.update_stream();
        discv4_service.spawn();

        // Optional discv5 update event listener if v5 is enabled
        let mut discv5_updates = None;

        if self.v5 {
            info!("Starting discv5");
            let config = Config::builder(self.addr).build();
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

    fn network_secret(&self) -> eyre::Result<SecretKey> {
        match &self.p2p_secret_key {
            Some(path) => Ok(get_secret_key(path)?),
            None => Ok(rng_secret_key()),
        }
    }
}
