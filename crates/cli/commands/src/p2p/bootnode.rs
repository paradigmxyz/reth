//! Standalone bootnode command

use clap::Parser;
use reth_cli_util::{get_secret_key, load_secret_key::rng_secret_key};
use reth_discv4::{DiscoveryUpdate, Discv4, Discv4Config};
use reth_discv5::{
    discv5::{self, Event, ListenConfig},
    Config, Discv5, DEFAULT_DISCOVERY_V5_PORT,
};
use reth_net_nat::NatResolver;
use reth_network_peers::NodeRecord;
use secp256k1::SecretKey;
use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    path::PathBuf,
};
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

    /// NAT resolution method (any|none|upnp|publicip|extip:\<IP\>).
    ///
    /// Can be repeated with one IPv4 and one IPv6 `extip:<IP>` to advertise a dual-stack discv5
    /// ENR (discv4 binds a single socket and always advertises only the `--addr` family).
    #[arg(long, default_value = "any")]
    pub nat: Vec<NatResolver>,

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
        let nat = self.resolved_nat()?;

        let config = self.discv4_config(&nat);

        let (_discv4, mut discv4_service) = Discv4::bind(self.addr, local_enr, sk, config).await?;

        info!("Started discv4 at address: {local_enr:?}");

        let mut discv4_updates = discv4_service.update_stream();
        discv4_service.spawn();

        // Optional discv5 update event listener if v5 is enabled
        let mut discv5_updates = None;
        // Keep the discv5 service alive for the event loop lifetime.
        let mut _discv5 = None;

        if self.v5 {
            info!("Starting discv5");
            let config = self.discv5_config(&nat);
            let (discv5, updates) = Discv5::start(&sk, config).await?;
            log_discv5_enr(&discv5);
            _discv5 = Some(discv5);
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

    /// Resolves the configured `--nat` values into the addresses to advertise.
    ///
    /// A single value behaves like the rest of the node: the resolver drives discv4's external IP
    /// resolution, and any fixed IP is also advertised in the discv5 ENR. Two `extip:<IP>` values
    /// (one per family) advertise a dual-stack discv5 record; discv4 advertises only the family
    /// matching `--addr`, since it binds a single socket.
    fn resolved_nat(&self) -> eyre::Result<BootnodeNat> {
        match self.nat.as_slice() {
            [] => Ok(BootnodeNat::single(NatResolver::Any, self.addr.port())),
            [nat] => Ok(BootnodeNat::single(nat.clone(), self.addr.port())),
            [first, second] => {
                let first_ip = fixed_external_ip(first)?;
                let second_ip = fixed_external_ip(second)?;

                if first_ip.is_ipv4() == second_ip.is_ipv4() {
                    eyre::bail!("repeated --nat requires one IPv4 and one IPv6 extip:<IP> value");
                }

                // discv4 binds a single socket, so its resolver must use the `--addr` family.
                let primary_ip =
                    if first_ip.is_ipv4() == self.addr.is_ipv4() { first_ip } else { second_ip };

                Ok(BootnodeNat {
                    resolver: NatResolver::ExternalIp(primary_ip),
                    advertised_ips: vec![first_ip, second_ip],
                })
            }
            _ => eyre::bail!("--nat can be provided at most twice"),
        }
    }

    fn discv4_config(&self, nat: &BootnodeNat) -> Discv4Config {
        Discv4Config::builder().external_ip_resolver(Some(nat.resolver.clone())).build()
    }

    fn discv5_config(&self, nat: &BootnodeNat) -> Config {
        let mut builder = Config::builder(self.addr);

        // Bind every family we advertise (plus the `--addr` family). Without a listen socket of a
        // given family, `build_local_enr` drops that family's advertised IP even though setting it
        // disables `enr_update` — which would leave discv5 with no usable record.
        let bind_ipv4 = self.addr.is_ipv4() || nat.advertised_ips.iter().any(IpAddr::is_ipv4);
        let bind_ipv6 = self.addr.is_ipv6() || nat.advertised_ips.iter().any(IpAddr::is_ipv6);
        if bind_ipv4 && bind_ipv6 {
            let listen = ListenConfig::DualStack {
                ipv4: Ipv4Addr::UNSPECIFIED,
                ipv4_port: DEFAULT_DISCOVERY_V5_PORT,
                ipv6: Ipv6Addr::UNSPECIFIED,
                ipv6_port: DEFAULT_DISCOVERY_V5_PORT,
            };
            builder = builder.discv5_config(discv5::ConfigBuilder::new(listen).build());
        }

        for ip in &nat.advertised_ips {
            builder = builder.advertised_ip(*ip);
        }

        builder.build()
    }
}

/// Resolved `--nat` configuration for the bootnode.
#[derive(Debug)]
struct BootnodeNat {
    /// Resolver driving discv4's external IP resolution.
    resolver: NatResolver,
    /// Fixed IPs to advertise in the discv5 ENR (one per family).
    advertised_ips: Vec<IpAddr>,
}

impl BootnodeNat {
    fn single(resolver: NatResolver, port: u16) -> Self {
        let advertised_ips = resolver.clone().as_external_ip(port).into_iter().collect();
        Self { resolver, advertised_ips }
    }
}

fn fixed_external_ip(nat: &NatResolver) -> eyre::Result<IpAddr> {
    match nat {
        NatResolver::ExternalIp(ip) => Ok(*ip),
        _ => eyre::bail!("--nat can only be repeated with extip:<IP> values"),
    }
}

fn log_discv5_enr(discv5: &Discv5) {
    let enr = discv5.local_enr();
    info!(
        id = ?enr.node_id(),
        ip4 = ?enr.ip4(),
        ip6 = ?enr.ip6(),
        udp4 = ?enr.udp4(),
        udp6 = ?enr.udp6(),
        tcp4 = ?enr.tcp4(),
        tcp6 = ?enr.tcp6(),
        "(Discv5) advertised endpoints",
    );
    info!("(Discv5) enr: {enr}");
    if let Some(record) = discv5.node_record() {
        info!("(Discv5) enode: {record}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_discv5::build_local_enr;

    #[test]
    fn repeated_nat_uses_matching_addr_family_as_primary() {
        let command = Command::parse_from([
            "reth",
            "--addr",
            "[::]:30303",
            "--nat",
            "extip:1.2.3.4",
            "--nat",
            "extip:2001:db8::1",
        ]);

        let nat = command.resolved_nat().unwrap();

        // discv4 binds the `--addr` (IPv6) family, so its resolver uses the IPv6 extip; both
        // families are still advertised by discv5.
        assert_eq!(nat.resolver, NatResolver::ExternalIp("2001:db8::1".parse().unwrap()));
        assert_eq!(nat.advertised_ips.len(), 2);
    }

    #[test]
    fn repeated_nat_requires_extip_values() {
        let command = Command::parse_from(["reth", "--nat", "any", "--nat", "extip:2001:db8::1"]);

        assert!(command.resolved_nat().is_err());
    }

    #[test]
    fn repeated_nat_requires_distinct_ip_families() {
        let command =
            Command::parse_from(["reth", "--nat", "extip:1.2.3.4", "--nat", "extip:5.6.7.8"]);

        assert!(command.resolved_nat().is_err());
    }

    #[test]
    fn single_extip_is_advertised() {
        let command =
            Command::parse_from(["reth", "--addr", "0.0.0.0:30301", "--nat", "extip:1.2.3.4"]);

        let nat = command.resolved_nat().unwrap();

        assert_eq!(nat.advertised_ips, vec!["1.2.3.4".parse::<IpAddr>().unwrap()]);
    }

    #[test]
    fn discv5_advertises_single_extip_of_other_family_than_addr() {
        // A v6 extip under a v4 `--addr` must still be bound and advertised, not silently dropped.
        let command = Command::parse_from([
            "reth",
            "--addr",
            "0.0.0.0:30301",
            "--v5",
            "--nat",
            "extip:2001:db8::1",
        ]);
        let nat = command.resolved_nat().unwrap();
        let config = command.discv5_config(&nat);
        let sk = SecretKey::from_byte_array(&[1u8; 32]).unwrap();

        let (enr, _, _, _) = build_local_enr(&sk, &config);

        assert_eq!(enr.ip6(), Some("2001:db8::1".parse().unwrap()));
        assert_eq!(enr.udp6(), Some(DEFAULT_DISCOVERY_V5_PORT));
    }

    #[test]
    fn discv4_config_advertises_only_the_addr_family() {
        // discv4 binds a single socket, so it must not advertise the secondary (IPv6) family it
        // cannot serve; the IPv4 resolver drives discv4 and only discv5 carries both families.
        let command = Command::parse_from([
            "reth",
            "--addr",
            "0.0.0.0:30303",
            "--nat",
            "extip:1.2.3.4",
            "--nat",
            "extip:2001:db8::1",
        ]);
        let nat = command.resolved_nat().unwrap();
        let config = command.discv4_config(&nat);

        assert_eq!(nat.resolver, NatResolver::ExternalIp("1.2.3.4".parse().unwrap()));
        assert!(config.additional_eip868_rlp_pairs.is_empty());
    }

    #[test]
    fn discv5_config_advertises_dual_stack_nat_endpoints() {
        let command = Command::parse_from([
            "reth",
            "--addr",
            "0.0.0.0:30301",
            "--v5",
            "--nat",
            "extip:1.2.3.4",
            "--nat",
            "extip:2001:db8::1",
        ]);
        let nat = command.resolved_nat().unwrap();
        let config = command.discv5_config(&nat);
        let sk = SecretKey::from_byte_array(&[1u8; 32]).unwrap();

        let (enr, _, _, _) = build_local_enr(&sk, &config);

        assert_eq!(enr.ip4(), Some("1.2.3.4".parse().unwrap()));
        assert_eq!(enr.ip6(), Some("2001:db8::1".parse().unwrap()));
        assert_eq!(enr.udp4(), Some(DEFAULT_DISCOVERY_V5_PORT));
        assert_eq!(enr.udp6(), Some(DEFAULT_DISCOVERY_V5_PORT));
        // Discovery runs over UDP, advertised for both families above. The ENR's TCP hint is the
        // single rlpx/`--addr` port and is emitted only for that family (there is no second-family
        // TCP socket to point at); a discovery-only bootnode never serves rlpx regardless.
        assert_eq!(enr.tcp4(), Some(30301));
        assert_eq!(enr.tcp6(), None);
    }
}
