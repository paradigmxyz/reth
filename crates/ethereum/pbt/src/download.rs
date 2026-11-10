//! Bounded eth/71 retrieval of missing migration BALs.

use alloy_primitives::{Bytes, B256};
use eyre::WrapErr;
use reth_ethereum::{
    network::p2p::{BalRequirement, BlockAccessListsClient},
    provider::RawBal,
};
use std::time::Duration;

/// Maximum wait for a network client or one BAL response.
pub(super) const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_REQUEST_ATTEMPTS: usize = 3;

/// Retrieves one BAL against a trusted header commitment, rejecting exhausted retries or budgets.
pub(super) async fn download_bal(
    client: &impl BlockAccessListsClient,
    hash: B256,
    commitment: B256,
    remaining_bytes: usize,
) -> eyre::Result<Bytes> {
    let mut last_error = eyre::eyre!("PBT catch-up BAL is unavailable from peers");
    for _ in 0..MAX_REQUEST_ATTEMPTS {
        let response = match tokio::time::timeout(
            REQUEST_TIMEOUT,
            client.get_block_access_lists_with_requirement(vec![hash], BalRequirement::Mandatory),
        )
        .await
        {
            Ok(Ok(response)) => response,
            Ok(Err(error)) => {
                last_error = error.into();
                continue;
            }
            Err(error) => {
                last_error = error.into();
                continue;
            }
        };
        let (peer, mut lists) = response.split();
        if lists.0.len() != 1 {
            client.report_bad_message(peer);
            last_error = eyre::eyre!("PBT catch-up peer returned an invalid BAL response length");
            continue;
        }
        let Some(bytes) = lists.0.pop().flatten() else { continue };
        if let Err(error) = RawBal::new(bytes.clone()).ensure_hash(commitment) {
            client.report_bad_message(peer);
            last_error = error.into();
            continue;
        }
        if bytes.len() > remaining_bytes {
            return Err(BalByteBudgetExceeded.into());
        }
        return Ok(bytes);
    }
    Err(last_error).wrap_err("PBT catch-up could not retrieve an authenticated BAL")
}

#[derive(Debug, thiserror::Error)]
#[error("PBT catch-up BAL byte budget exceeded")]
pub(super) struct BalByteBudgetExceeded;

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::keccak256;
    use reth_ethereum::network::{
        api::PeerId,
        p2p::{
            download::DownloadClient,
            error::{PeerRequestResult, RequestError},
            priority::Priority,
            test_utils::TestFullBlockClient,
            BlockAccessLists,
        },
    };
    use std::{
        collections::VecDeque,
        future::{ready, Ready},
        sync::{
            atomic::{AtomicUsize, Ordering},
            Mutex,
        },
    };

    #[tokio::test]
    async fn downloaded_bal_is_authenticated_and_bounded() {
        let client = TestFullBlockClient::default();
        let hash = B256::repeat_byte(1);
        let bytes = Bytes::from_static(&[0xc0]);
        let commitment = keccak256(&bytes);
        client.insert_access_list(hash, bytes.clone());
        assert_eq!(download_bal(&client, hash, commitment, 1).await.unwrap(), bytes);
        assert_eq!(
            download_bal(&client, hash, commitment, 0).await.unwrap_err().to_string(),
            "PBT catch-up BAL byte budget exceeded"
        );
        let error = download_bal(&client, hash, B256::ZERO, 1).await.unwrap_err();
        let mismatch = error.downcast_ref::<alloy_eip7928::BlockAccessListHashMismatch>().unwrap();
        assert_eq!(mismatch.expected, B256::ZERO);
        assert_eq!(mismatch.computed, commitment);
    }

    #[tokio::test]
    async fn unavailable_bal_returns_an_error_after_bounded_attempts() {
        let client = TestFullBlockClient::default();
        let error = download_bal(&client, B256::ZERO, B256::ZERO, 1).await.unwrap_err();
        assert_eq!(error.to_string(), "PBT catch-up could not retrieve an authenticated BAL");
        assert_eq!(error.root_cause().to_string(), "PBT catch-up BAL is unavailable from peers");
    }

    #[tokio::test]
    async fn failed_and_malformed_responses_are_retried() {
        let bytes = Bytes::from_static(&[0xc0]);
        let client = ScriptedClient {
            responses: Mutex::new(VecDeque::from([
                Err(RequestError::Timeout),
                Ok((PeerId::ZERO, BlockAccessLists(vec![])).into()),
                Ok((PeerId::ZERO, BlockAccessLists(vec![Some(bytes.clone())])).into()),
            ])),
            ..Default::default()
        };
        assert_eq!(
            download_bal(&client, B256::ZERO, keccak256(&bytes), bytes.len()).await.unwrap(),
            bytes
        );
        assert_eq!(client.requests.load(Ordering::Relaxed), MAX_REQUEST_ATTEMPTS);
        assert_eq!(client.bad_messages.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn unauthenticated_bytes_do_not_exhaust_the_budget() {
        let bytes = Bytes::from_static(&[0xc0]);
        let client = ScriptedClient {
            responses: Mutex::new(VecDeque::from([
                Ok((PeerId::ZERO, BlockAccessLists(vec![Some(Bytes::from_static(&[0xc1, 0x80]))]))
                    .into()),
                Ok((PeerId::ZERO, BlockAccessLists(vec![None])).into()),
                Ok((PeerId::ZERO, BlockAccessLists(vec![Some(bytes.clone())])).into()),
            ])),
            ..Default::default()
        };
        assert_eq!(
            download_bal(&client, B256::ZERO, keccak256(&bytes), bytes.len()).await.unwrap(),
            bytes
        );
        assert_eq!(client.requests.load(Ordering::Relaxed), MAX_REQUEST_ATTEMPTS);
        assert_eq!(client.bad_messages.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn exhausted_attempts_preserve_the_request_error() {
        let client = ScriptedClient {
            responses: Mutex::new(VecDeque::from([
                Err(RequestError::Timeout),
                Err(RequestError::BadResponse),
                Err(RequestError::ConnectionDropped),
            ])),
            ..Default::default()
        };
        let error = download_bal(&client, B256::ZERO, B256::ZERO, 1).await.unwrap_err();
        assert_eq!(error.downcast_ref::<RequestError>(), Some(&RequestError::ConnectionDropped));
        assert_eq!(client.requests.load(Ordering::Relaxed), MAX_REQUEST_ATTEMPTS);
        assert_eq!(client.bad_messages.load(Ordering::Relaxed), 0);
    }

    #[derive(Debug, Default)]
    struct ScriptedClient {
        responses: Mutex<VecDeque<PeerRequestResult<BlockAccessLists>>>,
        requests: AtomicUsize,
        bad_messages: AtomicUsize,
    }

    impl DownloadClient for ScriptedClient {
        fn report_bad_message(&self, _peer_id: PeerId) {
            self.bad_messages.fetch_add(1, Ordering::Relaxed);
        }

        fn num_connected_peers(&self) -> usize {
            1
        }
    }

    impl BlockAccessListsClient for ScriptedClient {
        type Output = Ready<PeerRequestResult<BlockAccessLists>>;

        fn get_block_access_lists_with_priority_and_requirement(
            &self,
            _hashes: Vec<B256>,
            _priority: Priority,
            requirement: BalRequirement,
        ) -> Self::Output {
            assert_eq!(requirement, BalRequirement::Mandatory);
            self.requests.fetch_add(1, Ordering::Relaxed);
            ready(self.responses.lock().unwrap().pop_front().expect("request count is bounded"))
        }
    }
}
