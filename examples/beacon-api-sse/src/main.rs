//! Example of how to subscribe to beacon chain events via SSE.
//!
//! See also [ethereum-beacon-API eventstream](https://ethereum.github.io/beacon-APIs/#/Events/eventstream)
//!
//! Run with
//!
//! ```sh
//! cargo run -p example-beacon-api-sse -- node
//! ```
//!
//! This launches a regular reth instance and subscribes to payload attributes event stream.
//!
//! **NOTE**: This expects that the CL client is running an http server on `localhost:5052` and is
//! configured to emit payload attributes events.
//!
//! See lighthouse beacon Node API: <https://lighthouse-book.sigmaprime.io/api_bn.html#beacon-node-api>

#![warn(unused_crate_dependencies)]

use alloy_rpc_types_beacon::events::PayloadAttributesEvent;
use bytes::BytesMut;
use clap::Parser;
use futures_util::stream::StreamExt;
use reth_ethereum::{
    cli::{chainspec::EthereumChainSpecParser, interface::Cli},
    node::EthereumNode,
};
use std::net::{IpAddr, Ipv4Addr};
use tracing::{info, warn};

fn main() {
    Cli::<EthereumChainSpecParser, BeaconEventsConfig>::parse()
        .run(async move |builder, args| {
            let handle = builder.node(EthereumNode::default()).launch().await?;

            handle.node.task_executor.spawn_task(args.run());

            handle.wait_for_node_exit().await
        })
        .unwrap();
}

/// Our custom cli args extension that adds one flag to reth default CLI.
#[derive(Debug, Clone, clap::Parser)]
struct BeaconEventsConfig {
    /// Beacon Node http server address
    #[arg(long = "cl.addr", default_value_t = IpAddr::V4(Ipv4Addr::LOCALHOST))]
    pub cl_addr: IpAddr,
    /// Beacon Node http server port to listen on
    #[arg(long = "cl.port", default_value_t = 5052)]
    pub cl_port: u16,
}

impl BeaconEventsConfig {
    /// Returns the http url of the beacon node
    pub fn http_base_url(&self) -> String {
        format!("http://{}:{}", self.cl_addr, self.cl_port)
    }

    /// Returns the URL to the events endpoint
    pub fn events_url(&self) -> String {
        format!("{}/eth/v1/events", self.http_base_url())
    }

    /// Service that subscribes to beacon chain payload attributes events
    async fn run(self) {
        let client = reqwest::Client::new();
        let payloads_url = format!("{}?topics=payload_attributes", self.events_url());

        loop {
            let response = match client.get(&payloads_url).send().await {
                Ok(resp) => match resp.error_for_status() {
                    Ok(resp) => resp,
                    Err(err) => {
                        warn!(?err, "Beacon SSE endpoint returned error status, retrying in 5s");
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                        continue;
                    }
                },
                Err(err) => {
                    warn!(?err, "Failed to connect to beacon SSE endpoint, retrying in 5s");
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    continue;
                }
            };

            let mut stream = response.bytes_stream();
            let mut buf = BytesMut::new();

            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(bytes) => {
                        buf.extend_from_slice(&bytes);

                        while let Some((pos, delim_len)) = find_event_boundary(&buf) {
                            let event_bytes = buf.split_to(pos);
                            let _ = buf.split_to(delim_len);

                            let event_str = String::from_utf8_lossy(&event_bytes);
                            if let Some(data) = extract_sse_data(&event_str) {
                                match serde_json::from_str::<PayloadAttributesEvent>(&data) {
                                    Ok(event) => {
                                        info!("Received payload attributes: {:?}", event);
                                    }
                                    Err(err) => {
                                        warn!(?err, "Failed to deserialize payload attributes");
                                    }
                                }
                            }
                        }
                    }
                    Err(err) => {
                        warn!(?err, "SSE stream error, reconnecting in 5s");
                        break;
                    }
                }
            }

            warn!("SSE stream ended, reconnecting in 5s");
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    }
}

/// Finds the position and length of the first double-newline SSE event delimiter in the buffer.
fn find_event_boundary(buf: &[u8]) -> Option<(usize, usize)> {
    for i in 0..buf.len().saturating_sub(1) {
        if buf[i] == b'\n' && buf[i + 1] == b'\n' {
            return Some((i, 2));
        }
        if i + 3 < buf.len() &&
            buf[i] == b'\r' &&
            buf[i + 1] == b'\n' &&
            buf[i + 2] == b'\r' &&
            buf[i + 3] == b'\n'
        {
            return Some((i, 4));
        }
    }
    None
}

/// Extracts the `data:` field value from an SSE event string.
///
/// Multiple `data:` lines are joined with newlines per the SSE spec.
fn extract_sse_data(event: &str) -> Option<String> {
    let mut out = String::new();
    for line in event.lines() {
        if let Some(data) = line.strip_prefix("data:") {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(data.trim_start());
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_config() {
        let args = BeaconEventsConfig::try_parse_from(["reth"]);
        assert!(args.is_ok());
    }

    #[test]
    fn find_boundary_lf() {
        let buf = b"data: hello\n\ndata: world\n\n";
        let (pos, len) = find_event_boundary(buf).unwrap();
        assert_eq!(pos, 11);
        assert_eq!(len, 2);
    }

    #[test]
    fn find_boundary_crlf() {
        let buf = b"data: hello\r\n\r\ndata: world\r\n\r\n";
        let (pos, len) = find_event_boundary(buf).unwrap();
        assert_eq!(pos, 11);
        assert_eq!(len, 4);
    }

    #[test]
    fn extract_single_data_line() {
        let event = "event: payload_attributes\ndata: {\"key\":\"value\"}";
        let data = extract_sse_data(event).unwrap();
        assert_eq!(data, "{\"key\":\"value\"}");
    }

    #[test]
    fn extract_multiple_data_lines() {
        let event = "data: line1\ndata: line2";
        let data = extract_sse_data(event).unwrap();
        assert_eq!(data, "line1\nline2");
    }
}
