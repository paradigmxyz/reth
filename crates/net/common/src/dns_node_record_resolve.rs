//! Resolves a [DNSNodeRecord] to a [NodeRecord] by looking up the record in the DNS system.

use reth_network_types::{DNSNodeRecord, NodeRecord};
use std::io::Error;
use url::Host;

/// Resolves the host in a [DNSNodeRecord] to an IP address, returning a [NodeRecord]. If the domain
/// cannot be resolved, an error is returned.
///
/// If the host is already an IP address, the [NodeRecord] is returned immediately.
pub async fn resolve_dns_node_record(node_record: DNSNodeRecord) -> Result<NodeRecord, Error> {
    let domain = match node_record.host {
        Host::Ipv4(ip) => {
            let id = node_record.id;
            let tcp_port = node_record.tcp_port;
            let udp_port = node_record.udp_port;

            return Ok(NodeRecord { address: ip.into(), id, tcp_port, udp_port })
        }
        Host::Ipv6(ip) => {
            let id = node_record.id;
            let tcp_port = node_record.tcp_port;
            let udp_port = node_record.udp_port;

            return Ok(NodeRecord { address: ip.into(), id, tcp_port, udp_port })
        }
        Host::Domain(domain) => domain,
    };

    let mut ips = tokio::net::lookup_host(domain).await?;
    let ip = ips
        .next()
        .ok_or_else(|| Error::new(std::io::ErrorKind::AddrNotAvailable, "No IP found"))?;

    Ok(NodeRecord {
        address: ip.ip(),
        id: node_record.id,
        tcp_port: node_record.tcp_port,
        udp_port: node_record.udp_port,
    })
}
