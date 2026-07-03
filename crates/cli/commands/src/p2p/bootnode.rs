//! Standalone bootnode command

use clap::Parser;
use reth_cli_util::{get_secret_key, load_secret_key::rng_secret_key};
use reth_discv4::{DiscoveryUpdate, Discv4, Discv4Config};
use reth_discv5::{
    discv5::{self, Event, ListenConfig},
    Config, Discv5,
};
use reth_net_nat::NatResolver;
use reth_network_peers::NodeRecord;
use secp256k1::SecretKey;
use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    path::PathBuf,
    sync::Arc,
};
use tokio::{net::UdpSocket, select, sync::mpsc};
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

    /// Run a discv5 bootnode alongside discv4 on the same UDP port (`--addr`).
    #[arg(long)]
    pub v5: bool,
}

impl Command {
    /// Execute the bootnode command.
    pub async fn execute(self) -> eyre::Result<()> {
        info!("Bootnode started with config: {self:?}");

        let sk = self.network_secret()?;
        // A discovery-only bootnode serves no RLPx, so advertise TCP port 0
        // (`enode://…@<ip>:0?discport=<udp>`).
        let local_enr = NodeRecord::from_secret_key(self.addr, &sk).with_tcp_port(0);
        let nat = self.resolved_nat()?;

        let discv4_config = self.discv4_config(&nat);

        // Keep the discv5 service and its forwarder task alive for the event loop lifetime.
        let mut discv5_updates = None;
        let mut _discv5 = None;
        let mut _discv5_forwarder = None;

        // In v5 mode discv4 and discv5 share a single UDP socket: discv5 owns the read loop and
        // discv4 packets surface as `UnrecognizedFrame`s forwarded to its ingress.
        let (_discv4, mut discv4_service) = if self.v5 {
            let shared_socket = bind_socket(self.addr).await?;
            // Actual port, so an ephemeral `--addr` port (`:0`) still yields one shared port.
            let shared_port = shared_socket.local_addr()?.port();

            let (discv4, discv4_service, mut ingress) =
                Discv4::bind_shared(shared_socket.clone(), local_enr, sk, discv4_config)?;
            info!("Started discv4 (shared port) at address: {local_enr:?}");

            // Hand discv5 the shared socket and bind the opposite family (if advertised) on the
            // same port.
            let mut discv5_config = self.discv5_config(&nat);
            let discv5_cfg = discv5_config.discv5_config_mut();
            let (mut ipv4, mut ipv6) = (None, None);
            if self.addr.is_ipv4() {
                ipv4 = Some(shared_socket);
                if let Some(mut addr) = reth_discv5::config::ipv6(&discv5_cfg.listen_config) {
                    addr.set_port(shared_port);
                    ipv6 = Some(bind_socket(SocketAddr::V6(addr)).await?);
                }
            } else {
                ipv6 = Some(shared_socket);
                if let Some(mut addr) = reth_discv5::config::ipv4(&discv5_cfg.listen_config) {
                    addr.set_port(shared_port);
                    ipv4 = Some(bind_socket(SocketAddr::V4(addr)).await?);
                }
            }
            discv5_cfg.listen_config = ListenConfig::FromSockets { ipv4, ipv6 };

            info!("Starting discv5 (shared port)");
            let (discv5, mut updates) = Discv5::start(&sk, discv5_config).await?;
            log_discv5_enr(&discv5);

            // A dedicated task drains discv5 events so discv4 frame forwarding is independent of
            // the logging loop; dropping log events under load beats stalling frame forwarding.
            let (tx, rx) = mpsc::channel(updates.max_capacity());
            let forwarder = tokio::spawn(async move {
                while let Some(event) = updates.recv().await {
                    match event {
                        Event::UnrecognizedFrame(frame) => {
                            ingress.handle_packet(&frame.packet, frame.src_address).await;
                        }
                        // Only sessions are logged; don't let other events fill the channel.
                        event @ Event::SessionEstablished(..) => {
                            if let Err(mpsc::error::TrySendError::Closed(_)) = tx.try_send(event) {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
            });

            _discv5 = Some(discv5);
            discv5_updates = Some(rx);
            _discv5_forwarder = Some(forwarder);

            (discv4, discv4_service)
        } else {
            let (discv4, discv4_service) =
                Discv4::bind(self.addr, local_enr, sk, discv4_config).await?;
            info!("Started discv4 at address: {local_enr:?}");
            (discv4, discv4_service)
        };

        let mut discv4_updates = discv4_service.update_stream();
        discv4_service.spawn();

        // event info loop for logging
        loop {
            select! {
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
                update = async {
                    if let Some(updates) = &mut discv5_updates {
                        updates.recv().await
                    } else {
                        futures::future::pending().await
                    }
                } => {
                    match update {
                        Some(Event::SessionEstablished(enr, _)) => {
                            info!("(Discv5) new peer added, peer_id={:?}", enr.id());
                        }
                        Some(_) => {}
                        None => {
                            info!("(Discv5) update stream ended.");
                            break;
                        }
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
            [nat] => {
                let single = BootnodeNat::single(nat.clone(), self.addr.port());
                // A static IP (`extip`/`extaddr`) of the opposite family to `--addr` can't drive
                // discv4 (it binds only the `--addr` family): advertise it via discv5 only, and
                // use `None` rather than `Any` so discv4 doesn't silently auto-resolve an address
                // the operator never provided.
                if let [ip] = single.advertised_ips[..] &&
                    ip.is_ipv4() != self.addr.is_ipv4()
                {
                    return Ok(BootnodeNat {
                        resolver: NatResolver::None,
                        advertised_ips: vec![ip],
                    });
                }
                Ok(single)
            }
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
        // A discovery-only bootnode serves no RLPx, so the ENR advertises `tcp=0` (enode
        // `:0?discport=<udp>`); the discovery listen port is set from `--addr` below.
        let mut builder = Config::builder(SocketAddr::new(self.addr.ip(), 0));

        // discv5 shares discv4's UDP port.
        let port = self.addr.port();

        // Bind every family we advertise (plus the `--addr` family). Without a listen socket of a
        // given family, `build_local_enr` drops that family's advertised IP even though setting it
        // disables `enr_update` — which would leave discv5 with no usable record.
        let bind_ipv4 = self.addr.is_ipv4() || nat.advertised_ips.iter().any(IpAddr::is_ipv4);
        let bind_ipv6 = self.addr.is_ipv6() || nat.advertised_ips.iter().any(IpAddr::is_ipv6);
        let listen = if bind_ipv4 && bind_ipv6 {
            ListenConfig::DualStack {
                ipv4: Ipv4Addr::UNSPECIFIED,
                ipv4_port: port,
                ipv6: Ipv6Addr::UNSPECIFIED,
                ipv6_port: port,
            }
        } else if bind_ipv6 {
            ListenConfig::Ipv6 { ip: Ipv6Addr::UNSPECIFIED, port }
        } else {
            ListenConfig::Ipv4 { ip: Ipv4Addr::UNSPECIFIED, port }
        };
        builder = builder.discv5_config(discv5::ConfigBuilder::new(listen).build());

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

/// Binds a UDP socket for shared discv4/discv5 use.
///
/// IPv6 sockets are bound with `IPV6_V6ONLY=true` so an IPv4 sibling socket on the same port
/// doesn't clash, matching how discv5 binds its `DualStack` sockets.
async fn bind_socket(addr: SocketAddr) -> eyre::Result<Arc<UdpSocket>> {
    let socket = match addr {
        SocketAddr::V4(_) => UdpSocket::bind(addr).await?,
        SocketAddr::V6(_) => {
            use socket2::{Domain, Protocol, Socket, Type};
            let socket = Socket::new(Domain::IPV6, Type::DGRAM, Some(Protocol::UDP))?;
            socket.set_only_v6(true)?;
            socket.set_nonblocking(true)?;
            socket.bind(&addr.into())?;
            UdpSocket::from_std(socket.into())?
        }
    };
    Ok(Arc::new(socket))
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
            "0.0.0.0:45678",
            "--v5",
            "--nat",
            "extip:2001:db8::1",
        ]);
        let nat = command.resolved_nat().unwrap();
        let config = command.discv5_config(&nat);
        let sk = SecretKey::from_byte_array(&[1u8; 32]).unwrap();

        let (enr, _, _, _) = build_local_enr(&sk, &config);

        // discv5 shares discv4's port, so it advertises the `--addr` port.
        assert_eq!(enr.ip6(), Some("2001:db8::1".parse().unwrap()));
        assert_eq!(enr.udp6(), Some(45678));
    }

    #[test]
    fn discv5_config_single_stack_ipv4_uses_addr_port() {
        // A single-family (v4-only) bootnode: discv5 listens/advertises the `--addr` port, v4 only.
        let command = Command::parse_from([
            "reth",
            "--addr",
            "0.0.0.0:45678",
            "--v5",
            "--nat",
            "extip:1.2.3.4",
        ]);
        let nat = command.resolved_nat().unwrap();
        let config = command.discv5_config(&nat);
        let sk = SecretKey::from_byte_array(&[1u8; 32]).unwrap();

        let (enr, _, _, _) = build_local_enr(&sk, &config);

        assert_eq!(enr.ip4(), Some("1.2.3.4".parse().unwrap()));
        assert_eq!(enr.udp4(), Some(45678));
        assert_eq!(enr.udp6(), None);
    }

    #[test]
    fn discv5_config_single_stack_ipv6_uses_addr_port() {
        // A single-family (v6-only) bootnode: discv5 listens/advertises the `--addr` port, v6 only.
        let command = Command::parse_from([
            "reth",
            "--addr",
            "[::]:45678",
            "--v5",
            "--nat",
            "extip:2001:db8::1",
        ]);
        let nat = command.resolved_nat().unwrap();
        let config = command.discv5_config(&nat);
        let sk = SecretKey::from_byte_array(&[1u8; 32]).unwrap();

        let (enr, _, _, _) = build_local_enr(&sk, &config);

        assert_eq!(enr.ip6(), Some("2001:db8::1".parse().unwrap()));
        assert_eq!(enr.udp6(), Some(45678));
        assert_eq!(enr.udp4(), None);
    }

    #[test]
    fn single_opposite_family_extip_does_not_drive_discv4() {
        // A single v6 extip under a v4 `--addr` must not become discv4's external IP (discv4 binds
        // only the v4 shared socket); it is still advertised via discv5.
        let command = Command::parse_from([
            "reth",
            "--addr",
            "0.0.0.0:45678",
            "--v5",
            "--nat",
            "extip:2001:db8::1",
        ]);
        let nat = command.resolved_nat().unwrap();

        assert_eq!(nat.resolver, NatResolver::None);
        assert_eq!(nat.advertised_ips, vec!["2001:db8::1".parse::<IpAddr>().unwrap()]);
    }

    #[tokio::test]
    async fn shared_socket_binds_both_families_on_one_port() {
        // A v4 and a v6 socket must bind the SAME port (V6ONLY prevents the clash). The probes
        // skip single-stack hosts; past them, a regressed V6ONLY must fail loudly.
        let (Ok(probe4), Ok(probe6)) = (
            bind_socket("0.0.0.0:0".parse().unwrap()).await,
            bind_socket("[::]:0".parse().unwrap()).await,
        ) else {
            return;
        };
        drop((probe4, probe6));

        let v4 = bind_socket("0.0.0.0:0".parse().unwrap()).await.unwrap();
        let port = v4.local_addr().unwrap().port();
        let v6 = bind_socket(format!("[::]:{port}").parse().unwrap()).await.unwrap();
        assert_eq!(v6.local_addr().unwrap().port(), port);
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
            "0.0.0.0:45678",
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

        // discv5 shares discv4's port, so both families advertise the `--addr` port.
        assert_eq!(enr.ip4(), Some("1.2.3.4".parse().unwrap()));
        assert_eq!(enr.ip6(), Some("2001:db8::1".parse().unwrap()));
        assert_eq!(enr.udp4(), Some(45678));
        assert_eq!(enr.udp6(), Some(45678));
        // A discovery-only bootnode serves no RLPx, so the ENR advertises `tcp=0`.
        assert_eq!(enr.tcp4(), Some(0));
        assert_eq!(enr.tcp6(), None);

        // The derived enode renders as `…@<ip>:0?discport=<udp>`.
        let record = NodeRecord::try_from(&enr).unwrap();
        assert_eq!(record.tcp_port, 0);
        assert_eq!(record.udp_port, 45678);
    }
}
