use reth_eth_wire::BlockBody;
use reth_interfaces::p2p::bodies::response::BlockResponse;
use reth_primitives::{SealedBlock, SealedHeader, H256};
use std::collections::HashMap;

pub(crate) fn zip_blocks<'a>(
    headers: impl Iterator<Item = &'a SealedHeader>,
    bodies: &mut HashMap<H256, BlockBody>,
) -> Vec<BlockResponse> {
    headers
        .into_iter()
        .map(|header| {
            let body = bodies.remove(&header.hash()).expect("body exists");
            if header.is_empty() {
                BlockResponse::Empty(header.clone())
            } else {
                BlockResponse::Full(SealedBlock {
                    header: header.clone(),
                    body: body.transactions,
                    ommers: body.ommers.into_iter().map(|o| o.seal()).collect(),
                })
            }
        })
        .collect()
}
