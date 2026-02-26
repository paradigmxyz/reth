use std::{
    ops::RangeInclusive,
    pin::Pin,
    task::{ready, Context, Poll},
};

use crate::{download::DownloadClient, error::PeerRequestResult, priority::Priority};
use alloy_primitives::B256;
use auto_impl::auto_impl;
use futures::{Future, FutureExt};
use reth_eth_wire_types::BlockAccessLists;
use reth_primitives_traits::BlockBody;

#[auto_impl(&, Arc, Box)]
pub trait BlockAccessListsClient: DownloadClient {
    type Output: Future<Output = PeerRequestResult<BlockAccessLists>> + Send + Sync + Unpin;

    fn get_block_access_lists(&self, hashes: Vec<B256>) -> Self::Output;
    fn get_block_access_lists_with_priority(
        &self,
        hashes: Vec<B256>,
        priority: Priority,
    ) -> Self::Output;
}
