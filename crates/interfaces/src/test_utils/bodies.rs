use crate::p2p::bodies::{client::BodiesClient, error::BodiesClientError};
use async_trait::async_trait;
use reth_eth_wire::BlockBody;
use reth_primitives::H256;
use std::fmt::{Debug, Formatter};

/// A test client for fetching bodies
pub struct TestBodiesClient<F>
where
    F: Fn(H256) -> Result<BlockBody, BodiesClientError>,
{
    /// The function that is called on each body request.
    pub responder: F,
}

impl<F> Debug for TestBodiesClient<F>
where
    F: Fn(H256) -> Result<BlockBody, BodiesClientError>,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestBodiesClient").finish()
    }
}

#[async_trait]
impl<F> BodiesClient for TestBodiesClient<F>
where
    F: Fn(H256) -> Result<BlockBody, BodiesClientError> + Send + Sync,
{
    async fn get_block_body(&self, hash: H256) -> Result<BlockBody, BodiesClientError> {
        (self.responder)(hash)
    }
}
