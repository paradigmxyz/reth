//! Snap sync request handler (server-side).
//!
//! Handles incoming snap protocol requests from peers, serving account ranges,
//! storage ranges, bytecodes, and trie nodes from the local database.
//!
//! Modeled after [`EthRequestHandler`](reth_network::eth_requests::EthRequestHandler).

use alloy_consensus::constants::KECCAK_EMPTY;
use alloy_primitives::Bytes;
use alloy_rlp::Encodable;
use alloy_trie::EMPTY_ROOT_HASH;
use futures::StreamExt;
use reth_db_api::{
    cursor::{DbCursorRO, DbDupCursorRO},
    tables,
    transaction::DbTx,
};
use reth_eth_wire_types::snap::*;
use reth_network_p2p::error::RequestResult;
use reth_network_peers::PeerId;
use reth_primitives_traits::Account;
use reth_storage_api::{DBProvider, DatabaseProviderFactory};
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::sync::oneshot;
use tracing::{debug, trace};

/// Maximum number of accounts to serve per request.
const MAX_ACCOUNTS_SERVE: usize = 1024;

/// Maximum number of storage slots to serve per account per request.
const MAX_STORAGE_SERVE: usize = 1024;

/// Maximum number of bytecodes to serve per request.
const MAX_BYTECODES_SERVE: usize = 1024;

/// Maximum response size (2MB, matching eth limit).
const SOFT_RESPONSE_LIMIT: usize = 2 * 1024 * 1024;

/// Handles incoming snap protocol requests from peers.
///
/// This is spawned as a background service and polled to process requests.
#[derive(Debug)]
#[must_use = "Handler does nothing unless polled."]
pub struct SnapRequestHandler<F> {
    /// Provider factory for DB access.
    provider_factory: F,
    /// Incoming snap requests.
    incoming_requests: tokio_stream::wrappers::ReceiverStream<IncomingSnapRequest>,
}

impl<F> SnapRequestHandler<F> {
    /// Create a new instance.
    pub fn new(
        provider_factory: F,
        incoming: tokio::sync::mpsc::Receiver<IncomingSnapRequest>,
    ) -> Self {
        Self {
            provider_factory,
            incoming_requests: tokio_stream::wrappers::ReceiverStream::new(incoming),
        }
    }
}

impl<F> SnapRequestHandler<F>
where
    F: DatabaseProviderFactory,
{
    /// Handle a `GetAccountRange` request.
    fn on_account_range_request(
        &self,
        peer_id: PeerId,
        request: GetAccountRangeMessage,
        response: oneshot::Sender<RequestResult<AccountRangeMessage>>,
    ) {
        trace!(target: "net::snap", ?peer_id, ?request.starting_hash, ?request.limit_hash, "Received GetAccountRange");

        let mut accounts = Vec::new();

        let Ok(provider) = self.provider_factory.database_provider_ro() else {
            let _ = response.send(Ok(AccountRangeMessage {
                request_id: request.request_id,
                accounts,
                proof: vec![],
            }));
            return;
        };

        let tx = provider.tx_ref();
        let Ok(mut cursor) = tx.cursor_read::<tables::HashedAccounts>() else {
            let _ = response.send(Ok(AccountRangeMessage {
                request_id: request.request_id,
                accounts,
                proof: vec![],
            }));
            return;
        };

        let limit = (request.response_bytes as usize).min(SOFT_RESPONSE_LIMIT);
        let mut total_bytes = 0usize;

        if let Ok(walker) = cursor.walk(Some(request.starting_hash)) {
            for entry in walker {
                let Ok((hash, account)) = entry else { break };
                if hash > request.limit_hash {
                    break;
                }

                let slim = encode_account(&account);
                total_bytes += 32 + slim.len();
                accounts.push(AccountData { hash, body: slim });

                if accounts.len() >= MAX_ACCOUNTS_SERVE || total_bytes >= limit {
                    break;
                }
            }
        }

        trace!(target: "net::snap", ?peer_id, num_accounts = accounts.len(), total_bytes, "Serving GetAccountRange");

        let _ = response.send(Ok(AccountRangeMessage {
            request_id: request.request_id,
            accounts,
            // TODO: add merkle proofs
            proof: vec![],
        }));
    }

    /// Handle a `GetStorageRanges` request.
    fn on_storage_ranges_request(
        &self,
        peer_id: PeerId,
        request: GetStorageRangesMessage,
        response: oneshot::Sender<RequestResult<StorageRangesMessage>>,
    ) {
        trace!(target: "net::snap", ?peer_id, num_accounts = request.account_hashes.len(), "Received GetStorageRanges");

        let mut all_slots = Vec::new();

        let Ok(provider) = self.provider_factory.database_provider_ro() else {
            let _ = response.send(Ok(StorageRangesMessage {
                request_id: request.request_id,
                slots: all_slots,
                proof: vec![],
            }));
            return;
        };

        let tx = provider.tx_ref();
        let Ok(mut cursor) = tx.cursor_dup_read::<tables::HashedStorages>() else {
            let _ = response.send(Ok(StorageRangesMessage {
                request_id: request.request_id,
                slots: all_slots,
                proof: vec![],
            }));
            return;
        };

        let limit = (request.response_bytes as usize).min(SOFT_RESPONSE_LIMIT);
        let mut total_bytes = 0usize;

        for (i, account_hash) in request.account_hashes.iter().enumerate() {
            let mut account_slots = Vec::new();

            // For the first account, use the request's starting_hash.
            // For subsequent accounts, start from the beginning.
            let start = if i == 0 { request.starting_hash } else { Default::default() };

            if let Ok(walker) = cursor.walk_dup(Some(*account_hash), Some(start)) {
                for entry in walker {
                    let Ok((_, storage_entry)) = entry else { break };
                    if storage_entry.key > request.limit_hash {
                        break;
                    }

                    let mut value_buf = Vec::new();
                    storage_entry.value.encode(&mut value_buf);
                    total_bytes += 32 + value_buf.len();

                    account_slots.push(StorageData {
                        hash: storage_entry.key,
                        data: Bytes::from(value_buf),
                    });

                    if account_slots.len() >= MAX_STORAGE_SERVE || total_bytes >= limit {
                        break;
                    }
                }
            }

            all_slots.push(account_slots);

            if total_bytes >= limit {
                break;
            }
        }

        trace!(target: "net::snap", ?peer_id, num_accounts = all_slots.len(), total_bytes, "Serving GetStorageRanges");

        let _ = response.send(Ok(StorageRangesMessage {
            request_id: request.request_id,
            slots: all_slots,
            // TODO: add boundary proofs for partial ranges
            proof: vec![],
        }));
    }

    /// Handle a `GetByteCodes` request.
    fn on_byte_codes_request(
        &self,
        peer_id: PeerId,
        request: GetByteCodesMessage,
        response: oneshot::Sender<RequestResult<ByteCodesMessage>>,
    ) {
        trace!(target: "net::snap", ?peer_id, num_hashes = request.hashes.len(), "Received GetByteCodes");

        let mut codes = Vec::new();

        let Ok(provider) = self.provider_factory.database_provider_ro() else {
            let _ = response.send(Ok(ByteCodesMessage { request_id: request.request_id, codes }));
            return;
        };

        let tx = provider.tx_ref();
        let limit = (request.response_bytes as usize).min(SOFT_RESPONSE_LIMIT);
        let mut total_bytes = 0usize;

        for hash in &request.hashes {
            if *hash == KECCAK_EMPTY {
                continue;
            }

            let Ok(Some(bytecode)) = tx.get::<tables::Bytecodes>(*hash) else {
                continue;
            };

            let raw = bytecode.original_bytes();
            total_bytes += raw.len();
            codes.push(raw);

            if codes.len() >= MAX_BYTECODES_SERVE || total_bytes >= limit {
                break;
            }
        }

        trace!(target: "net::snap", ?peer_id, num_codes = codes.len(), total_bytes, "Serving GetByteCodes");

        let _ = response.send(Ok(ByteCodesMessage { request_id: request.request_id, codes }));
    }

    /// Handle a `GetTrieNodes` request.
    fn on_trie_nodes_request(
        &self,
        peer_id: PeerId,
        request: GetTrieNodesMessage,
        response: oneshot::Sender<RequestResult<TrieNodesMessage>>,
    ) {
        debug!(target: "net::snap", ?peer_id, num_paths = request.paths.len(), "Received GetTrieNodes (stub)");

        // TODO: implement trie node lookups from AccountsTrie / StoragesTrie tables
        let _ =
            response.send(Ok(TrieNodesMessage { request_id: request.request_id, nodes: vec![] }));
    }
}

/// Encode an [`Account`] into slim RLP format (as `TrieAccount`).
///
/// Accounts in the snap protocol are exchanged as RLP-encoded `TrieAccount`.
/// Since we don't know the true storage root when reading from `HashedAccounts`,
/// we use `EMPTY_ROOT_HASH` as a placeholder.
fn encode_account(account: &Account) -> Bytes {
    let trie_account = account.into_trie_account(EMPTY_ROOT_HASH);
    let mut buf = Vec::new();
    trie_account.encode(&mut buf);
    Bytes::from(buf)
}

/// Incoming snap request variants delegated by the network.
#[derive(Debug)]
pub enum IncomingSnapRequest {
    /// Request for an account range.
    GetAccountRange {
        /// The peer that sent the request.
        peer_id: PeerId,
        /// The request payload.
        request: GetAccountRangeMessage,
        /// Channel to send the response.
        response: oneshot::Sender<RequestResult<AccountRangeMessage>>,
    },
    /// Request for storage slot ranges.
    GetStorageRanges {
        /// The peer that sent the request.
        peer_id: PeerId,
        /// The request payload.
        request: GetStorageRangesMessage,
        /// Channel to send the response.
        response: oneshot::Sender<RequestResult<StorageRangesMessage>>,
    },
    /// Request for contract bytecodes.
    GetByteCodes {
        /// The peer that sent the request.
        peer_id: PeerId,
        /// The request payload.
        request: GetByteCodesMessage,
        /// Channel to send the response.
        response: oneshot::Sender<RequestResult<ByteCodesMessage>>,
    },
    /// Request for trie nodes.
    GetTrieNodes {
        /// The peer that sent the request.
        peer_id: PeerId,
        /// The request payload.
        request: GetTrieNodesMessage,
        /// Channel to send the response.
        response: oneshot::Sender<RequestResult<TrieNodesMessage>>,
    },
}

impl<F> Future for SnapRequestHandler<F>
where
    F: DatabaseProviderFactory + Unpin,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        loop {
            match this.incoming_requests.poll_next_unpin(cx) {
                Poll::Ready(Some(incoming)) => match incoming {
                    IncomingSnapRequest::GetAccountRange { peer_id, request, response } => {
                        this.on_account_range_request(peer_id, request, response);
                    }
                    IncomingSnapRequest::GetStorageRanges { peer_id, request, response } => {
                        this.on_storage_ranges_request(peer_id, request, response);
                    }
                    IncomingSnapRequest::GetByteCodes { peer_id, request, response } => {
                        this.on_byte_codes_request(peer_id, request, response);
                    }
                    IncomingSnapRequest::GetTrieNodes { peer_id, request, response } => {
                        this.on_trie_nodes_request(peer_id, request, response);
                    }
                },
                Poll::Ready(None) => return Poll::Ready(()),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{keccak256, B256, U256};
    use alloy_rlp::Decodable;
    use reth_db_api::transaction::DbTxMut;
    use reth_provider::test_utils::create_test_provider_factory;
    use reth_storage_api::{DBProvider, DatabaseProviderFactory};
    use tokio::sync::mpsc;

    #[test]
    fn test_encode_account_roundtrip() {
        let account = Account {
            nonce: 10,
            balance: U256::from(500),
            bytecode_hash: Some(B256::repeat_byte(0xAB)),
        };
        let encoded = encode_account(&account);
        assert!(!encoded.is_empty());

        let decoded = alloy_trie::TrieAccount::decode(&mut encoded.as_ref()).unwrap();
        assert_eq!(decoded.nonce, 10);
        assert_eq!(decoded.balance, U256::from(500));
        assert_eq!(decoded.code_hash, B256::repeat_byte(0xAB));
    }

    #[tokio::test]
    async fn test_account_range_empty_db() {
        let factory = create_test_provider_factory();
        let (tx, rx) = mpsc::channel(10);
        let handler = SnapRequestHandler::new(factory, rx);

        let handle = tokio::spawn(handler);

        let (resp_tx, resp_rx) = oneshot::channel();
        tx.send(IncomingSnapRequest::GetAccountRange {
            peer_id: PeerId::random(),
            request: GetAccountRangeMessage {
                request_id: 1,
                root_hash: B256::ZERO,
                starting_hash: B256::ZERO,
                limit_hash: B256::repeat_byte(0xFF),
                response_bytes: 512 * 1024,
            },
            response: resp_tx,
        })
        .await
        .unwrap();

        let result = resp_rx.await.unwrap().unwrap();
        assert!(result.accounts.is_empty());

        drop(tx);
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_byte_codes_request() {
        let factory = create_test_provider_factory();

        // Write bytecodes into the DB
        let code = Bytes::from(vec![0x60, 0x00, 0x60, 0x00, 0xFD]); // PUSH0 PUSH0 REVERT
        let code_hash = keccak256(&code);
        {
            let provider = factory.database_provider_rw().unwrap();
            let bytecode = reth_primitives_traits::Bytecode::new_raw(code.clone());
            provider.tx_ref().put::<tables::Bytecodes>(code_hash, bytecode).unwrap();
            provider.commit().unwrap();
        }

        let (tx, rx) = mpsc::channel(10);
        let handler = SnapRequestHandler::new(factory, rx);
        let handle = tokio::spawn(handler);

        let (resp_tx, resp_rx) = oneshot::channel();
        tx.send(IncomingSnapRequest::GetByteCodes {
            peer_id: PeerId::random(),
            request: GetByteCodesMessage {
                request_id: 2,
                hashes: vec![code_hash],
                response_bytes: 512 * 1024,
            },
            response: resp_tx,
        })
        .await
        .unwrap();

        let result = resp_rx.await.unwrap().unwrap();
        assert_eq!(result.codes.len(), 1);
        assert_eq!(result.codes[0], code);

        drop(tx);
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_storage_ranges_empty() {
        let factory = create_test_provider_factory();
        let (tx, rx) = mpsc::channel(10);
        let handler = SnapRequestHandler::new(factory, rx);
        let handle = tokio::spawn(handler);

        let (resp_tx, resp_rx) = oneshot::channel();
        tx.send(IncomingSnapRequest::GetStorageRanges {
            peer_id: PeerId::random(),
            request: GetStorageRangesMessage {
                request_id: 3,
                root_hash: B256::ZERO,
                account_hashes: vec![B256::repeat_byte(0x01)],
                starting_hash: B256::ZERO,
                limit_hash: B256::repeat_byte(0xFF),
                response_bytes: 512 * 1024,
            },
            response: resp_tx,
        })
        .await
        .unwrap();

        let result = resp_rx.await.unwrap().unwrap();
        // One entry per requested account, but the inner vec is empty since no storage exists
        assert_eq!(result.slots.len(), 1);
        assert!(result.slots[0].is_empty());

        drop(tx);
        handle.await.unwrap();
    }
}
