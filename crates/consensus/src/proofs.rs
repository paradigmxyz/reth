use hash_db::Hasher;
use plain_hasher::PlainHasher;
use reth_primitives::{Bytes, Header, Log, Receipt, TransactionSigned, H256};
use reth_rlp::Encodable;
use rlp::RlpStream;
use sha3::{Digest, Keccak256};
use triehash::sec_trie_root;

#[derive(Default, Debug, Clone, PartialEq, Eq)]
struct KeccakHasher;
impl Hasher for KeccakHasher {
    type Out = H256;
    type StdHasher = PlainHasher;
    const LENGTH: usize = 32;
    fn hash(x: &[u8]) -> Self::Out {
        let out = Keccak256::digest(x);
        // TODO make more performant, H256 from slice is not good enought.
        H256::from_slice(out.as_slice())
    }
}

/// Calculate Transaction root. Iterate over transaction and create merkle trie of
/// (rlp(index),encoded(tx)) pairs.
pub fn calculate_transaction_root<'a>(
    transactions: impl IntoIterator<Item = &'a TransactionSigned>,
) -> H256 {
    sec_trie_root::<KeccakHasher, _, _, _>(
        transactions
            .into_iter()
            .enumerate()
            .map(|(index, tx)| {
                // TODO replace with reth-rlp
                let mut stream = RlpStream::new();
                stream.append(&index);
                let mut bytes = Vec::new();
                tx.encode(&mut bytes);
                (stream.out().freeze().into(), bytes)
            })
            .collect::<Vec<(Bytes, Vec<u8>)>>(),
    )
}

/// Create receipt root for header
pub fn calculate_receipt_root<'a>(receipts: impl IntoIterator<Item = &'a Receipt>) -> H256 {
    sec_trie_root::<KeccakHasher, _, _, _>(
        receipts
            .into_iter()
            .enumerate()
            .map(|(index, receipt)| {
                let mut stream = RlpStream::new();
                stream.append(&index);
                let mut bytes = Vec::new();
                receipt.encode(&mut bytes);
                (stream.out().freeze().into(), bytes)
            })
            .collect::<Vec<(Bytes, Vec<u8>)>>(),
    )
}

/// Create log hash for header
pub fn calculate_log_root<'a>(logs: impl IntoIterator<Item = &'a Log>) -> H256 {
    //https://github.com/ethereum/go-ethereum/blob/356bbe343a30789e77bb38f25983c8f2f2bfbb47/cmd/evm/internal/t8ntool/execution.go#L255
    let mut stream = RlpStream::new();
    stream.begin_unbounded_list();
    for log in logs {
        stream.begin_list(3);
        stream.append(&log.address);
        stream.append_list(&log.topics);
        stream.append(&log.data);
    }
    stream.finalize_unbounded_list();
    let out = stream.out().freeze();

    let out = Keccak256::digest(out);
    H256::from_slice(out.as_slice())
}

/// Calculate hash for ommer/uncle headers
pub fn calculate_ommers_root<'a>(_ommers: impl IntoIterator<Item = &'a Header>) -> H256 {
    // RLP Encode
    let mut stream = RlpStream::new();
    stream.begin_unbounded_list();
    /* TODO
    for ommer in ommers {
        stream.append(ommer)
    }
     */
    stream.finalize_unbounded_list();
    let bytes = stream.out().freeze();
    let out = Keccak256::digest(bytes);
    H256::from_slice(out.as_slice())
}

// TODO state root

// TODO bloom
