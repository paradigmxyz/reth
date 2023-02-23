//! Common CLI utility functions.

use reth_interfaces::p2p::{
    headers::client::{HeadersClient, HeadersRequest},
    priority::Priority,
};
use reth_network::FetchClient;
use reth_primitives::{BlockHashOrNumber, HeadersDirection, SealedHeader};
use std::sync::Arc;

/// Get a single header from network
pub async fn get_single_header(
    client: Arc<FetchClient>,
    id: BlockHashOrNumber,
) -> eyre::Result<SealedHeader> {
    let request = HeadersRequest { direction: HeadersDirection::Rising, limit: 1, start: id };

    let (_, response) = client.get_headers_with_priority(request, Priority::High).await?.split();

    if response.len() != 1 {
        eyre::bail!("Invalid number of headers received. Expected: 1. Received: {}", response.len())
    }

    let header = response.into_iter().next().unwrap().seal_slow();

    let valid = match id {
        BlockHashOrNumber::Hash(hash) => header.hash() == hash,
        BlockHashOrNumber::Number(number) => header.number == number,
    };

    if !valid {
        eyre::bail!(
            "Received invalid header. Received: {:?}. Expected: {:?}",
            header.num_hash(),
            id
        );
    }

    Ok(header)
}
