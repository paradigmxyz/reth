use crate::{keccak256, Header, Log, Receipt, TransactionSigned, H256};
use hash_db::Hasher;
use hex_literal::hex;
use plain_hasher::PlainHasher;
use triehash::ordered_trie_root;

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
    ordered_trie_root::<KeccakHasher, _>(transactions.into_iter().map(|tx| {
        let mut tx_rlp = Vec::new();
        tx.encode_inner(&mut tx_rlp, false);
        tx_rlp
    }))
}

/// Calculates the receipt root for a header.
pub fn calculate_receipt_root<'a>(receipts: impl Iterator<Item = &'a Receipt>) -> H256 {
    ordered_trie_root::<KeccakHasher, _>(receipts.into_iter().map(|receipt| {
        let mut receipt_rlp = Vec::new();
        receipt.encode_inner(&mut receipt_rlp, false);
        receipt_rlp
    }))
}

/// Calculates the log root for headers.
pub fn calculate_log_root<'a>(logs: impl Iterator<Item = &'a Log> + Clone) -> H256 {
    //https://github.com/ethereum/go-ethereum/blob/356bbe343a30789e77bb38f25983c8f2f2bfbb47/cmd/evm/internal/t8ntool/execution.go#L255
    let mut logs_rlp = Vec::new();
    reth_rlp::encode_iter(logs, &mut logs_rlp);
    keccak256(logs_rlp)
}

/// Calculates the root hash for ommer/uncle headers.
pub fn calculate_ommers_root<'a>(ommers: impl Iterator<Item = &'a Header> + Clone) -> H256 {
    // RLP Encode
    let mut ommers_rlp = Vec::new();
    reth_rlp::encode_iter(ommers, &mut ommers_rlp);
    keccak256(ommers_rlp)
}

#[cfg(test)]
mod tests {

    use crate::{
        hex_literal::hex,
        proofs::{calculate_receipt_root, calculate_transaction_root},
        Block, Bloom, Log, Receipt, TxType, H160, H256,
    };
    use reth_rlp::Decodable;

    #[test]
    fn check_transaction_root() {
        let data = &hex!("f90262f901f9a092230ce5476ae868e98c7979cfc165a93f8b6ad1922acf2df62e340916efd49da01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347942adc25665018aa1fe0e6bc666dac8fc2697ff9baa02307107a867056ca33b5087e77c4174f47625e48fb49f1c70ced34890ddd88f3a08151d548273f6683169524b66ca9fe338b9ce42bc3540046c828fd939ae23bcba0c598f69a5674cae9337261b669970e24abc0b46e6d284372a239ec8ccbf20b0ab901000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000083020000018502540be40082a8618203e800a00000000000000000000000000000000000000000000000000000000000000000880000000000000000f863f861800a8405f5e10094100000000000000000000000000000000000000080801ba07e09e26678ed4fac08a249ebe8ed680bf9051a5e14ad223e4b2b9d26e0208f37a05f6e3f188e3e6eab7d7d3b6568f5eac7d687b08d307d3154ccd8c87b4630509bc0");
        let block_rlp = &mut data.as_slice();
        let block: Block = Block::decode(block_rlp).unwrap();

        let tx_root = calculate_transaction_root(block.body.iter());
        assert_eq!(block.transactions_root, tx_root, "Should be same");
    }

    #[test]
    fn check_receipt_root() {
        let logs = vec![Log { address: H160::zero(), topics: vec![], data: Default::default() }];
        let bloom =  Bloom(hex!("00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000001"));
        let receipt = Receipt {
            tx_type: TxType::EIP2930,
            success: true,
            cumulative_gas_used: 102068,
            bloom,
            logs,
        };
        let receipt = vec![receipt];
        let root = calculate_receipt_root(receipt.iter());
        assert_eq!(
            root,
            H256(hex!("fe70ae4a136d98944951b2123859698d59ad251a381abc9960fa81cae3d0d4a0"))
        );
    }
}
