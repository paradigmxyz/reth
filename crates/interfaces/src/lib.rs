#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! Reth interface bindings

/// Block Execution traits.
pub mod executor;

/// Consensus traits.
pub mod consensus;

/// Database traits.
pub mod db;
/// Traits that provide chain access.
pub mod provider;

/// P2P traits.
pub mod p2p;

/// Possible errors when interacting with the chain.
mod error;

pub use error::{Error, Result};

#[cfg(any(test, feature = "test-helpers"))]
pub mod test_utils {
    use crate::{
        consensus::{self, Consensus},
        p2p::headers::{
            client::{HeadersClient, HeadersRequest, HeadersStream},
            downloader::{DownloadError, Downloader},
        },
    };
    use std::{collections::HashSet, time::Duration};
    use tokio::sync::watch;

    use reth_primitives::{Header, HeaderLocked, H256, H512};
    use reth_rpc_types::engine::ForkchoiceState;

    #[derive(Debug)]
    pub struct TestDownloader {
        result: Result<Vec<HeaderLocked>, DownloadError>,
    }

    impl TestDownloader {
        pub fn new(result: Result<Vec<HeaderLocked>, DownloadError>) -> Self {
            Self { result }
        }
    }

    #[async_trait::async_trait]
    impl Downloader for TestDownloader {
        type Consensus = TestConsensus;
        type Client = TestHeadersClient;

        fn timeout(&self) -> Duration {
            Duration::from_millis(1000)
        }

        fn consensus(&self) -> &Self::Consensus {
            unimplemented!()
        }

        fn client(&self) -> &Self::Client {
            unimplemented!()
        }

        async fn download(
            &self,
            _: &HeaderLocked,
            _: &ForkchoiceState,
        ) -> Result<Vec<HeaderLocked>, DownloadError> {
            self.result.clone()
        }
    }

    #[derive(Debug, Clone)]
    pub struct TestHeadersClient;

    #[async_trait::async_trait]
    impl HeadersClient for TestHeadersClient {
        async fn update_status(&self, height: u64, hash: H256, td: H256) {}

        async fn send_header_request(&self, id: u64, request: HeadersRequest) -> HashSet<H512> {
            unimplemented!()
        }

        async fn stream_headers(&self) -> HeadersStream {
            unimplemented!()
        }
    }

    /// Consensus client impl for testing
    #[derive(Debug)]
    pub struct TestConsensus;

    #[async_trait::async_trait]
    impl Consensus for TestConsensus {
        fn fork_choice_state(&self) -> watch::Receiver<ForkchoiceState> {
            unimplemented!()
        }

        fn validate_header(
            &self,
            _header: &Header,
            _parent: &Header,
        ) -> Result<(), consensus::Error> {
            unimplemented!()
        }
    }
}
