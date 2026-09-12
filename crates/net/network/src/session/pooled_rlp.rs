//! Shared byte-bounded cache for version-specific blob transaction response encodings.

use alloy_primitives::{Bytes, B256};
use alloy_rlp::{Encodable, Header};
use parking_lot::Mutex;
use reth_eth_wire_types::{EncodableEth72PooledTransaction, EthMessageID, RawCapabilityMessage};
use reth_primitives_traits::SignedTransaction;
use schnellru::{LruMap, Unlimited};
use std::sync::Arc;

/// Shared by sessions, so repeated requests from different peers reuse the same encoding.
#[derive(Clone, Debug, Default)]
pub(super) struct PooledRlpCache(Arc<Mutex<Cache>>);

impl PooledRlpCache {
    pub(super) fn response<T: SignedTransaction + EncodableEth72PooledTransaction>(
        &self,
        request_id: u64,
        transactions: &[T],
        eth72: bool,
    ) -> RawCapabilityMessage {
        let mut payload = Vec::new();
        for tx in transactions {
            let length = if eth72 { tx.eth72_length() } else { tx.length() };
            // The length distinguishes pre/post-Osaka wrappers for the same signed transaction.
            let key = (*tx.tx_hash(), eth72, length);
            if tx.is_eip4844() &&
                let Some(encoded) = self.0.lock().entries.get(&key).cloned()
            {
                payload.extend_from_slice(&encoded);
                continue
            }
            let mut encoded = Vec::with_capacity(length);
            if eth72 {
                tx.encode_eth72(&mut encoded);
            } else {
                tx.encode(&mut encoded);
            }
            payload.extend_from_slice(&encoded);
            if tx.is_eip4844() {
                self.0.lock().insert(key, encoded.into());
            }
        }
        let list = Header { list: true, payload_length: payload.len() };
        let outer =
            Header { list: true, payload_length: request_id.length() + list.length_with_payload() };
        let mut out = Vec::with_capacity(outer.length_with_payload());
        outer.encode(&mut out);
        request_id.encode(&mut out);
        list.encode(&mut out);
        out.extend_from_slice(&payload);
        RawCapabilityMessage::eth(EthMessageID::PooledTransactions, out.into())
    }
}

#[derive(Debug)]
struct Cache {
    entries: LruMap<(B256, bool, usize), Bytes, Unlimited>,
    bytes: usize,
}
impl Default for Cache {
    fn default() -> Self {
        Self { entries: LruMap::new(Unlimited), bytes: 0 }
    }
}
impl Cache {
    const MAX_BYTES: usize = 16 * 1024 * 1024;
    fn insert(&mut self, key: (B256, bool, usize), value: Bytes) {
        if value.len() > Self::MAX_BYTES {
            return
        }
        if let Some(old) = self.entries.remove(&key) {
            self.bytes -= old.len();
        }
        self.bytes += value.len();
        self.entries.insert(key, value);
        while self.bytes > Self::MAX_BYTES || self.entries.len() > 4096 {
            if let Some((_, oldest)) = self.entries.pop_oldest() {
                self.bytes -= oldest.len();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{SignableTransaction, TxEip4844, TxEip4844WithSidecar};
    use alloy_eips::{
        eip4844::{Blob, Bytes48},
        eip7594::BlobTransactionSidecarEip7594,
    };
    use alloy_primitives::Signature;
    use reth_eth_wire_types::{message::RequestPair, PooledTransactions, PooledTransactionsEth72};
    use reth_ethereum_primitives::PooledTransactionVariant;

    #[test]
    fn cached_responses_match_both_wire_encodings_and_keep_request_ids() {
        let sidecar = BlobTransactionSidecarEip7594::new(
            vec![Blob::default()],
            vec![Bytes48::ZERO],
            vec![Bytes48::ZERO; 128],
        );
        let tx = TxEip4844WithSidecar {
            tx: TxEip4844 { blob_versioned_hashes: vec![B256::ZERO], ..Default::default() },
            sidecar: sidecar.into(),
        }
        .into_signed(Signature::test_signature());
        let transactions = vec![PooledTransactionVariant::Eip4844(tx)];
        let cache = PooledRlpCache::default();
        for request_id in [1, 128, 65536] {
            let full = cache.response(request_id, &transactions, false);
            assert_eq!(
                full.payload.as_ref(),
                alloy_rlp::encode(RequestPair {
                    request_id,
                    message: PooledTransactions(transactions.clone())
                })
            );
            let elided = cache.response(request_id, &transactions, true);
            assert_eq!(
                elided.payload.as_ref(),
                alloy_rlp::encode(RequestPair {
                    request_id,
                    message: PooledTransactionsEth72(transactions.clone())
                })
            );
            assert!(elided.payload.len() < full.payload.len());
        }
        assert_eq!(cache.0.lock().entries.len(), 2);
    }

    #[test]
    fn encoded_cache_enforces_byte_budget_on_replacement_and_eviction() {
        let mut cache = Cache::default();
        let first = (B256::ZERO, false, 10 * 1024 * 1024);
        let second = (B256::repeat_byte(1), false, 10 * 1024 * 1024);
        cache.insert(first, vec![0; first.2].into());
        cache.insert(first, vec![0; first.2].into());
        assert_eq!(cache.bytes, first.2);
        cache.insert(second, vec![0; second.2].into());
        assert_eq!(cache.bytes, second.2);
        assert!(cache.entries.peek(&first).is_none());
        assert_eq!(cache.entries.len(), 1);
    }
}
