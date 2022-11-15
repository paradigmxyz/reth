use crate::{keccak256, Bytes, Header, Log, Receipt, TransactionSigned, H256};
use ethers_core::utils::rlp::RlpStream;
use hash_db::Hasher;
use hex_literal::hex;
use plain_hasher::PlainHasher;
use reth_rlp::Encodable;
use triehash::sec_trie_root;

/// Keccak-256 hash of the RLP of an empty list, KEC("\xc0").
pub const EMPTY_LIST_HASH: H256 =
    H256(hex!("1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347"));

/// Root hash of an empty trie.
pub const EMPTY_ROOT: H256 =
    H256(hex!("56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421"));

/// A [Hasher] that calculates a keccak256 hash of the given data.
#[derive(Default, Debug, Clone, PartialEq, Eq)]
struct KeccakHasher;

impl Hasher for KeccakHasher {
    type Out = H256;
    type StdHasher = PlainHasher;

    const LENGTH: usize = 32;

    fn hash(x: &[u8]) -> Self::Out {
        keccak256(x)
    }
}

/// Calculate a transaction root.
///
/// Iterates over the given transactions and the merkle merkle trie root of
/// `(rlp(index), encoded(tx))` pairs.
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

/// Calculates the receipt root for a header.
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

/// Calculates the log root for a header.
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

    keccak256(out)
}

/// Calculates the root hash for ommer/uncle headers.
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
    keccak256(bytes)
}

// TODO state root

// TODO bloom
