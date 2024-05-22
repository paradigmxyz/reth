//! Resolves a [DNSNodeRecord] to a [NodeRecord] by looking up the record in the DNS system.

use reth_network_types::{DNSNodeRecord, NodeRecord};
use std::io::Error;
use url::Host;

/// Retry strategy for DNS lookups.
#[derive(Debug)]
pub struct RetryStrategy {
    /// The amount of time between retries.
    interval: u64,
    /// The number of retries.
    retries: usize,
}

/// Resolves the host in a [DNSNodeRecord] to an IP address, returning a [NodeRecord]. If the domain
/// cannot be resolved, an error is returned.
///
/// If the host is already an IP address, the [NodeRecord] is returned immediately.
///
/// An optional retry strategy can be provided via [RetryStrategy] to retry
/// the DNS lookup if it fails:
pub async fn resolve_dns_node_record(
    node_record: DNSNodeRecord,
    retry_strategy: Option<RetryStrategy>,
) -> Result<NodeRecord, Error> {
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

    // Lookip ipaddr from domain
    match lookup_host(&domain).await {
        Ok(ip) => Ok(NodeRecord {
            address: ip,
            id: node_record.id,
            tcp_port: node_record.tcp_port,
            udp_port: node_record.udp_port,
        }),
        Err(e) => {
            // Retry if strategy is provided
            if let Some(strat) = retry_strategy {
                let lookup = || async { lookup_host(&domain).await };
                let strat = tokio_retry::strategy::FixedInterval::from_millis(strat.interval)
                    .take(strat.retries);
                let ip = tokio_retry::Retry::spawn(strat, lookup).await?;
                Ok(NodeRecord {
                    address: ip,
                    id: node_record.id,
                    tcp_port: node_record.tcp_port,
                    udp_port: node_record.udp_port,
                })
            } else {
                Err(e)
            }
        }
    }
}

async fn lookup_host(domain: &str) -> Result<std::net::IpAddr, Error> {
    let mut ips = tokio::net::lookup_host(format!("{domain}:0")).await?;
    let ip = ips
        .next()
        .ok_or_else(|| Error::new(std::io::ErrorKind::AddrNotAvailable, "No IP found"))?;
    Ok(ip.ip())
}

mod tests {
    #[tokio::test]
    async fn test_resolve_dns_node_record() {
        use super::*;

        // Set up tests
        let tests = vec![
            ("localhost", None),
            ("localhost", Some(RetryStrategy { interval: 1000, retries: 3 })),
        ];

        // Run tests
        for (domain, retry_strategy) in tests {
            // Construct record
            let rec = DNSNodeRecord::new(
                url::Host::Domain(domain.to_owned()),
                30300,
                reth_network_types::PeerId::random(),
            );

            // Resolve domain and validate
            let rec = super::resolve_dns_node_record(rec.clone(), retry_strategy).await.unwrap();
            if let std::net::IpAddr::V4(addr) = rec.address {
                assert_eq!(addr, std::net::Ipv4Addr::new(127, 0, 0, 1))
            } else {
                panic!("Expected IPv4 address");
            }
        }
    }
}
