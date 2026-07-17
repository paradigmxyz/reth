//! Standalone bootnode command

use clap::Parser;
use reth_cli_util::{get_secret_key, load_secret_key::rng_secret_key};
use reth_discv4::{DiscoveryUpdate, Discv4, Discv4Config};
use reth_discv5::{
    discv5::{self, Event, ListenConfig},
    enr::EnrCombinedKeyWrapper,
    Config, Discv5,
};
use reth_net_nat::NatResolver;
use reth_network_peers::{id2pk, AnyNode, Enr, NodeRecord};
use secp256k1::SecretKey;
use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    path::PathBuf,
    sync::Arc,
};
use tokio::{net::UdpSocket, select};
use tokio_stream::StreamExt;
use tracing::{info, warn};

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
    /// Can be repeated with one IPv4 and one IPv6 `extip:<IP>` to advertise a dual-stack node.
    /// The enode advertises only the `--addr` family; the discv5 and EIP-868 ENRs carry both
    /// families, and with `--v5` discv4 also answers the opposite family on the shared port.
    #[arg(long, default_value = "any")]
    pub nat: Vec<NatResolver>,

    /// Also run discv5, sharing the discv4 UDP port (`--addr`).
    ///
    /// The opposite-family sibling socket is only bound when an `extip:<IP>` of that family is
    /// advertised via `--nat`; the default `--nat any` serves the `--addr` family only.
    #[arg(long)]
    pub v5: bool,

    /// Comma-separated nodes to seed discovery: `enode://` URLs with an IP host and/or signed
    /// `enr:` records. Each entry seeds discv4 and (with `--v5`) discv5.
    #[arg(long, value_delimiter = ',')]
    pub bootnodes: Vec<AnyNode>,
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
        let seeds = self.seed_nodes(&nat)?;

        for record in seeds.discv4_records() {
            if !self.serves_family(record.address, &nat) {
                warn!(
                    "No socket will be bound for the IP family of --bootnodes entry {record}; \
                     it will not be contacted (advertise a matching --nat extip with --v5)"
                );
            }
        }

        let discv4_config = self.discv4_config(&nat, &seeds);

        // In v5 mode discv4 and discv5 share a single UDP socket: discv5 owns the read loop and
        // discv4 packets surface as `UnrecognizedFrame`s forwarded to its ingress. The discv5
        // service must be kept alive for the event loop lifetime.
        let (_discv4, mut discv4_service, _discv5, mut discv5_forwarder) = if self.v5 {
            let shared_socket = bind_socket(self.addr).await?;
            // Actual port, so an ephemeral `--addr` port (`:0`) still yields one shared port.
            let shared_port = shared_socket.local_addr()?.port();

            // Hand discv5 the shared socket and bind the opposite family (if advertised) on the
            // same port. That sibling socket doubles as discv4's secondary-family egress, so
            // discv4 answers both families on the shared port.
            let mut discv5_config = self.discv5_config(&nat, &seeds);
            let discv5_cfg = discv5_config.discv5_config_mut();
            let (mut ipv4, mut ipv6) = (None, None);
            if self.addr.is_ipv4() {
                ipv4 = Some(shared_socket.clone());
                if let Some(mut addr) = reth_discv5::config::ipv6(&discv5_cfg.listen_config) {
                    addr.set_port(shared_port);
                    ipv6 = Some(bind_socket(SocketAddr::V6(addr)).await?);
                }
            } else {
                ipv6 = Some(shared_socket.clone());
                if let Some(mut addr) = reth_discv5::config::ipv4(&discv5_cfg.listen_config) {
                    addr.set_port(shared_port);
                    ipv4 = Some(bind_socket(SocketAddr::V4(addr)).await?);
                }
            }
            let secondary_socket = if self.addr.is_ipv4() { ipv6.clone() } else { ipv4.clone() };
            discv5_cfg.listen_config = ListenConfig::FromSockets { ipv4, ipv6 };

            let (discv4, discv4_service, mut ingress) =
                Discv4::bind_shared(shared_socket, secondary_socket, local_enr, sk, discv4_config)?;
            info!("Started discv4 (shared port) at address: {local_enr:?}");

            info!("Starting discv5 (shared port)");
            let (discv5, mut updates) = Discv5::start(&sk, discv5_config).await?;
            log_discv5_enr(&discv5);

            // A dedicated task forwards discv4 frames to their ingress and logs discv5 sessions.
            let forwarder = tokio::spawn(async move {
                while let Some(event) = updates.recv().await {
                    match event {
                        Event::UnrecognizedFrame(frame) => {
                            ingress.handle_packet(&frame.packet, frame.src_address).await
                        }
                        Event::SessionEstablished(enr, _) => {
                            info!("(Discv5) new peer added, peer_id={:?}", enr.id())
                        }
                        _ => {}
                    }
                }
                info!("(Discv5) update stream ended.");
            });

            (discv4, discv4_service, Some(discv5), Some(forwarder))
        } else {
            let (discv4, discv4_service) =
                Discv4::bind(self.addr, local_enr, sk, discv4_config).await?;
            info!("Started discv4 at address: {local_enr:?}");
            (discv4, discv4_service, None, None)
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
                // discv5 logging lives in the forwarder task; end the loop when it exits.
                _ = async {
                    if let Some(forwarder) = &mut discv5_forwarder {
                        let _ = forwarder.await;
                    } else {
                        futures::future::pending::<()>().await
                    }
                } => break,
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
    /// (one per family) advertise a dual-stack record; the enode advertises only the family
    /// matching `--addr`, since discv4's own record carries a single address.
    fn resolved_nat(&self) -> eyre::Result<BootnodeNat> {
        match self.nat.as_slice() {
            [] => Ok(BootnodeNat::single(NatResolver::Any, self.addr.port())),
            [nat] => {
                let mut single = BootnodeNat::single(nat.clone(), self.addr.port());
                // A static IP (`extip`/`extaddr`) of the opposite family to `--addr` can't drive
                // discv4 (it binds only the `--addr` family): advertise it via discv5 only, and
                // use `None` rather than `Any` so discv4 doesn't silently auto-resolve an address
                // the operator never provided.
                if let [ip] = single.advertised_ips[..] &&
                    ip.is_ipv4() != self.addr.is_ipv4()
                {
                    single.resolver = NatResolver::None;
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
            _ => {
                eyre::bail!("--nat can be provided at most twice");
            }
        }
    }

    fn discv4_config(&self, nat: &BootnodeNat, seeds: &BootnodeSeeds) -> Discv4Config {
        let mut builder = Discv4Config::builder();
        builder
            .external_ip_resolver(Some(nat.resolver.clone()))
            .add_boot_nodes(seeds.discv4_records());
        // the opposite-family address is only served with the `--v5` sibling socket
        if self.v5 &&
            let Some(ip) =
                nat.advertised_ips.iter().find(|ip| ip.is_ipv4() != self.addr.is_ipv4())
        {
            builder.secondary_advertised_ip(*ip);
        }
        builder.build()
    }

    /// Parses `--bootnodes`, keeping enode-sourced records separate from signed ENRs so an ENR
    /// entry is not seeded into discv5 twice (signed and again as an unsigned enode).
    fn seed_nodes(&self, nat: &BootnodeNat) -> eyre::Result<BootnodeSeeds> {
        let mut seeds = BootnodeSeeds::default();
        for node in &self.bootnodes {
            let record = match node {
                AnyNode::NodeRecord(record) => {
                    // v4-mapped hosts parse as V6; normalize so family routing is semantic
                    let record = record.into_ipv4_mapped();
                    id2pk(record.id).map_err(|err| {
                        eyre::eyre!("--bootnodes entry has an invalid node id ({err}): {node}")
                    })?;
                    seeds.enodes.push(record);
                    record
                }
                AnyNode::Enr(enr) => {
                    let record = self.enr_endpoint(enr, nat)?;
                    seeds.enr_records.push(record);
                    seeds.enrs.push(EnrCombinedKeyWrapper::from(enr.clone()).0);
                    record
                }
                node => eyre::bail!(
                    "--bootnodes entries must be enode URLs with an IP host or signed ENRs: {node}"
                ),
            };
            if record.udp_port == 0 {
                eyre::bail!("--bootnodes entry has no discovery port: {node}");
            }
        }
        Ok(seeds)
    }

    /// Derives an ENR seed's discv4 endpoint, preferring a family this bootnode serves over the
    /// conversion default (IPv4 first).
    fn enr_endpoint(&self, enr: &Enr<SecretKey>, nat: &BootnodeNat) -> eyre::Result<NodeRecord> {
        let record = NodeRecord::try_from(enr)
            .map_err(|err| eyre::eyre!("unusable --bootnodes ENR ({err}): {enr}"))?;
        if self.serves_family(record.address, nat) {
            return Ok(record)
        }
        let alt = if record.address.is_ipv4() {
            enr.ip6().map(IpAddr::from).zip(enr.udp6()).map(|e| (e, enr.tcp6()))
        } else {
            enr.ip4().map(IpAddr::from).zip(enr.udp4()).map(|e| (e, enr.tcp4()))
        };
        if let Some(((address, udp_port), tcp)) = alt &&
            self.serves_family(address, nat)
        {
            return Ok(NodeRecord { address, udp_port, tcp_port: tcp.unwrap_or(0), id: record.id })
        }
        // unservable either way; the startup warning covers it
        Ok(record)
    }

    /// Whether a socket of `ip`'s family will be bound: the `--addr` family always, the opposite
    /// family only when `--v5` binds its sibling socket (an address of that family is advertised).
    fn serves_family(&self, ip: IpAddr, nat: &BootnodeNat) -> bool {
        ip.is_ipv4() == self.addr.is_ipv4() ||
            (self.v5 && nat.advertised_ips.iter().any(|a| a.is_ipv4() == ip.is_ipv4()))
    }

    fn discv5_config(&self, nat: &BootnodeNat, seeds: &BootnodeSeeds) -> Config {
        // Discovery-only bootnode: no RLPx, so the ENR carries `tcp=0`; the discovery listen
        // port is set from `--addr` below.
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

        // discv5's bootstrap aborts on an ENR it cannot contact (AddNodeFailed) and wastes a
        // request_enr round-trip on an unreachable enode: seed only what the listen config serves
        let enrs = seeds.enrs.iter().filter(|enr| {
            let contactable = (bind_ipv4 && enr.udp4_socket().is_some()) ||
                (bind_ipv6 && enr.udp6_socket().is_some());
            if !contactable {
                warn!("--bootnodes ENR has no endpoint for a served IP family; discv5 will not dial it: {enr}");
            }
            contactable
        });
        let enodes = seeds.enodes.iter().filter(|record| {
            if record.address.is_ipv4() {
                bind_ipv4
            } else {
                bind_ipv6
            }
        });

        builder =
            builder.add_signed_boot_nodes(enrs.cloned()).add_unsigned_boot_nodes(enodes.copied());

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

/// Parsed `--bootnodes`, split by record kind.
#[derive(Debug, Default)]
struct BootnodeSeeds {
    /// `enode://` entries: seed discv4 directly and discv5 as unsigned boot nodes.
    enodes: Vec<NodeRecord>,
    /// Signed `enr:` entries: seed discv5.
    enrs: Vec<discv5::Enr>,
    /// Endpoints derived from `enrs`: seed discv4.
    enr_records: Vec<NodeRecord>,
}

impl BootnodeSeeds {
    /// All discv4 bootstrap records (enode entries + ENR-derived endpoints).
    fn discv4_records(&self) -> impl Iterator<Item = NodeRecord> + '_ {
        self.enodes.iter().chain(&self.enr_records).copied()
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
        _ => {
            eyre::bail!("--nat can only be repeated with extip:<IP> values");
        }
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
        let config = command.discv5_config(&nat, &command.seed_nodes(&nat).unwrap());
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
        let config = command.discv5_config(&nat, &command.seed_nodes(&nat).unwrap());
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
        let config = command.discv5_config(&nat, &command.seed_nodes(&nat).unwrap());
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
    fn discv4_config_serves_secondary_family_only_with_v5() {
        // The IPv4 resolver drives discv4's own record either way; the v6 address is only
        // advertised (EIP-868 ENR) when the `--v5` sibling socket exists to serve it.
        let args = [
            "reth",
            "--addr",
            "0.0.0.0:30303",
            "--nat",
            "extip:1.2.3.4",
            "--nat",
            "extip:2001:db8::1",
        ];

        let command = Command::parse_from(args);
        let nat = command.resolved_nat().unwrap();
        let config = command.discv4_config(&nat, &command.seed_nodes(&nat).unwrap());
        assert_eq!(nat.resolver, NatResolver::ExternalIp("1.2.3.4".parse().unwrap()));
        assert_eq!(config.secondary_advertised_ip, None);

        let command = Command::parse_from(args.iter().chain(&["--v5"]));
        let nat = command.resolved_nat().unwrap();
        let config = command.discv4_config(&nat, &command.seed_nodes(&nat).unwrap());
        assert_eq!(nat.resolver, NatResolver::ExternalIp("1.2.3.4".parse().unwrap()));
        assert_eq!(config.secondary_advertised_ip, Some("2001:db8::1".parse().unwrap()));
    }

    #[test]
    fn bootnodes_seed_discv4_and_discv5() {
        // The EIP-778 example record: ip 127.0.0.1, udp 30303, no tcp key (discovery-only).
        let enr = "enr:-IS4QHCYrYZbAKWCBRlAy5zzaDZXJBGkcnh4MHcBFZntXNFrdvJjX04jRzjzCBOonrkTfj499SZuOh8R33Ls8RRcy5wBgmlkgnY0gmlwhH8AAAGJc2VjcDI1NmsxoQPKY0yuDUmstAHYpMa2_oxVtw0RW_QAdpzBQA8yWM0xOIN1ZHCCdl8";
        let enode = "enode://d860a01f9722d78051619d1e2351aba3f43f943f6f00718d1b9baa4101932a1f5011f16bb2b1bb35db20d6fe28fa0bf09636d26a87d31de9ec6203eeedb1f666@1.2.3.4:0?discport=30303";

        let command = Command::parse_from(["reth", "--bootnodes", &format!("{enode},{enr}")]);
        let nat = command.resolved_nat().unwrap();
        let seeds = command.seed_nodes(&nat).unwrap();

        assert_eq!(seeds.enodes.len(), 1);
        assert_eq!(seeds.enodes[0].address, "1.2.3.4".parse::<IpAddr>().unwrap());
        assert_eq!(seeds.enodes[0].udp_port, 30303);
        assert_eq!(seeds.enodes[0].tcp_port, 0);
        // the ENR seeds discv4 too, with an absent tcp key meaning no RLPx — but stays out of
        // `enodes` so discv5 doesn't bootstrap it twice (signed + unsigned)
        assert_eq!(seeds.enr_records.len(), 1);
        assert_eq!(seeds.enr_records[0].address, "127.0.0.1".parse::<IpAddr>().unwrap());
        assert_eq!(seeds.enr_records[0].udp_port, 30303);
        assert_eq!(seeds.enr_records[0].tcp_port, 0);
        assert_eq!(seeds.enrs.len(), 1);
        assert_eq!(seeds.enrs[0].udp4(), Some(30303));

        let config = command.discv4_config(&command.resolved_nat().unwrap(), &seeds);
        assert_eq!(config.bootstrap_nodes.len(), 2);
    }

    #[test]
    fn bootnodes_normalize_and_validate_enodes() {
        let peer_id = "d860a01f9722d78051619d1e2351aba3f43f943f6f00718d1b9baa4101932a1f5011f16bb2b1bb35db20d6fe28fa0bf09636d26a87d31de9ec6203eeedb1f666";

        // a v4-mapped host normalizes to its semantic IPv4 family
        let command = Command::parse_from([
            "reth",
            "--bootnodes",
            &format!("enode://{peer_id}@[::ffff:9.9.9.9]:0?discport=30303"),
        ]);
        let seeds = command.seed_nodes(&command.resolved_nat().unwrap()).unwrap();
        assert_eq!(seeds.enodes[0].address, "9.9.9.9".parse::<IpAddr>().unwrap());

        // a zero discovery port can never be pinged
        let command =
            Command::parse_from(["reth", "--bootnodes", &format!("enode://{peer_id}@1.2.3.4:0")]);
        assert!(command.seed_nodes(&command.resolved_nat().unwrap()).is_err());

        // an id off the secp256k1 curve would be silently dropped by discv5's bootstrap
        let bad_id = "ff".repeat(64);
        let command = Command::parse_from([
            "reth",
            "--bootnodes",
            &format!("enode://{bad_id}@1.2.3.4:30303"),
        ]);
        assert!(command.seed_nodes(&command.resolved_nat().unwrap()).is_err());
    }

    #[test]
    fn enr_seed_prefers_served_family() {
        // Deterministic record (key 0xaa..aa): ip 9.9.9.9/udp 30303 + ip6 2001:db8::9/udp6 30304.
        let dual = "enr:-KG4QF9o9jc3N31d-QdtzGq_HRKxncK1dmwMPQv8upyE43wzaMXsUrFdXobKuBTEwRaxToiampTdT9gw0sbWMjrE8SoBgmlkgnY0gmlwhAkJCQmDaXA2kCABDbgAAAAAAAAAAAAAAAmJc2VjcDI1NmsxoQJqBKuY2eR3StgG4wLd3rY76ha1y18iPud0eOhhu1g-s4N1ZHCCdl-EdWRwNoJ2YA";

        // v6-primary: the discv4 seed must be the ENR's IPv6 endpoint, not the ip4-first default
        let command = Command::parse_from([
            "reth",
            "--addr",
            "[::]:30303",
            "--nat",
            "extip:2001:db8::1",
            "--bootnodes",
            dual,
        ]);
        let seeds = command.seed_nodes(&command.resolved_nat().unwrap()).unwrap();
        assert_eq!(seeds.enr_records[0].address, "2001:db8::9".parse::<IpAddr>().unwrap());
        assert_eq!(seeds.enr_records[0].udp_port, 30304);

        // v4-primary keeps the IPv4 endpoint
        let command = Command::parse_from([
            "reth",
            "--addr",
            "0.0.0.0:30303",
            "--nat",
            "extip:1.2.3.4",
            "--bootnodes",
            dual,
        ]);
        let seeds = command.seed_nodes(&command.resolved_nat().unwrap()).unwrap();
        assert_eq!(seeds.enr_records[0].address, "9.9.9.9".parse::<IpAddr>().unwrap());
        assert_eq!(seeds.enr_records[0].udp_port, 30303);
    }

    #[test]
    fn bootnodes_reject_endpointless_entries() {
        let peer_id = "d860a01f9722d78051619d1e2351aba3f43f943f6f00718d1b9baa4101932a1f5011f16bb2b1bb35db20d6fe28fa0bf09636d26a87d31de9ec6203eeedb1f666";

        let command = Command::parse_from(["reth", "--bootnodes", &format!("enode://{peer_id}")]);
        assert!(command.seed_nodes(&command.resolved_nat().unwrap()).is_err());
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
        let config = command.discv5_config(&nat, &command.seed_nodes(&nat).unwrap());
        let sk = SecretKey::from_byte_array(&[1u8; 32]).unwrap();

        let (enr, _, _, _) = build_local_enr(&sk, &config);

        // discv5 shares discv4's port, so both families advertise the `--addr` port.
        assert_eq!(enr.ip4(), Some("1.2.3.4".parse().unwrap()));
        assert_eq!(enr.ip6(), Some("2001:db8::1".parse().unwrap()));
        assert_eq!(enr.udp4(), Some(45678));
        assert_eq!(enr.udp6(), Some(45678));
        // No RLPx: tcp=0.
        assert_eq!(enr.tcp4(), Some(0));
        assert_eq!(enr.tcp6(), None);

        // The derived enode renders as `…@<ip>:0?discport=<udp>`.
        let record = NodeRecord::try_from(&enr).unwrap();
        assert_eq!(record.tcp_port, 0);
        assert_eq!(record.udp_port, 45678);
    }
}
