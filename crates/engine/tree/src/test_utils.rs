use reth_network_p2p::test_utils::TestFullBlockClient;
use reth_primitives::{BlockBody, SealedHeader};
use std::ops::Range;

pub(crate) fn insert_headers_into_client(
    client: &TestFullBlockClient,
    genesis_header: SealedHeader,
    range: Range<usize>,
) {
    let mut sealed_header = genesis_header;
    let body = BlockBody::default();
    for _ in range {
        let (mut header, hash) = sealed_header.split();
        // update to the next header
        header.parent_hash = hash;
        header.number += 1;
        header.timestamp += 1;
        sealed_header = header.seal_slow();
        client.insert(sealed_header.clone(), body.clone());
    }
}
