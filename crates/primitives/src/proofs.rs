//! Helper function for calculating Merkle proofs and hashes.

use crate::{
    constants::EMPTY_OMMER_ROOT_HASH, Header, Receipt, ReceiptWithBloom, ReceiptWithBloomRef,
    Request, TransactionSigned, Withdrawal,
};
use alloc::vec::Vec;
use alloy_eips::{eip2718::Encodable2718, eip7685::Encodable7685};
use alloy_primitives::{keccak256, B256};
use reth_trie_common::root::{ordered_trie_root, ordered_trie_root_with_encoder};

/// Calculate a transaction root.
///
/// `(rlp(index), encoded(tx))` pairs.
pub fn calculate_transaction_root<T>(transactions: &[T]) -> B256
where
    T: AsRef<TransactionSigned>,
{
    ordered_trie_root_with_encoder(transactions, |tx: &T, buf| tx.as_ref().encode_2718(buf))
}

/// Calculates the root hash of the withdrawals.
pub fn calculate_withdrawals_root(withdrawals: &[Withdrawal]) -> B256 {
    ordered_trie_root(withdrawals)
}

/// Calculates the receipt root for a header.
pub fn calculate_receipt_root(receipts: &[ReceiptWithBloom]) -> B256 {
    ordered_trie_root_with_encoder(receipts, |r, buf| r.encode_inner(buf, false))
}

/// Calculate [EIP-7685](https://eips.ethereum.org/EIPS/eip-7685) requests root.
///
/// NOTE: The requests are encoded as `id + request`
pub fn calculate_requests_root(requests: &[Request]) -> B256 {
    ordered_trie_root_with_encoder(requests, |item, buf| item.encode_7685(buf))
}

/// Calculates the receipt root for a header.
pub fn calculate_receipt_root_ref(receipts: &[ReceiptWithBloomRef<'_>]) -> B256 {
    ordered_trie_root_with_encoder(receipts, |r, buf| r.encode_inner(buf, false))
}

/// Calculates the receipt root for a header for the reference type of [Receipt].
///
/// NOTE: Prefer [`calculate_receipt_root`] if you have log blooms memoized.
pub fn calculate_receipt_root_no_memo(receipts: &[&Receipt]) -> B256 {
    ordered_trie_root_with_encoder(receipts, |r, buf| {
        ReceiptWithBloomRef::from(*r).encode_inner(buf, false)
    })
}

/// Calculates the root hash for ommer/uncle headers.
pub fn calculate_ommers_root(ommers: &[Header]) -> B256 {
    // Check if `ommers` list is empty
    if ommers.is_empty() {
        return EMPTY_OMMER_ROOT_HASH
    }
    // RLP Encode
    let mut ommers_rlp = Vec::new();
    alloy_rlp::encode_list(ommers, &mut ommers_rlp);
    keccak256(ommers_rlp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{constants::EMPTY_ROOT_HASH, Block};
    use alloy_genesis::GenesisAccount;
    use alloy_primitives::{b256, hex_literal::hex, Address, U256};
    use alloy_rlp::Decodable;
    use reth_chainspec::{HOLESKY, MAINNET, SEPOLIA};
    use reth_trie_common::root::{state_root_ref_unhashed, state_root_unhashed};
    use std::collections::HashMap;

    #[cfg(not(feature = "optimism"))]
    use crate::TxType;
    #[cfg(not(feature = "optimism"))]
    use alloy_primitives::{bloom, Log, LogData};

    #[test]
    fn check_transaction_root() {
        let data = &hex!("f90262f901f9a092230ce5476ae868e98c7979cfc165a93f8b6ad1922acf2df62e340916efd49da01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347942adc25665018aa1fe0e6bc666dac8fc2697ff9baa02307107a867056ca33b5087e77c4174f47625e48fb49f1c70ced34890ddd88f3a08151d548273f6683169524b66ca9fe338b9ce42bc3540046c828fd939ae23bcba0c598f69a5674cae9337261b669970e24abc0b46e6d284372a239ec8ccbf20b0ab901000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000083020000018502540be40082a8618203e800a00000000000000000000000000000000000000000000000000000000000000000880000000000000000f863f861800a8405f5e10094100000000000000000000000000000000000000080801ba07e09e26678ed4fac08a249ebe8ed680bf9051a5e14ad223e4b2b9d26e0208f37a05f6e3f188e3e6eab7d7d3b6568f5eac7d687b08d307d3154ccd8c87b4630509bc0");
        let block_rlp = &mut data.as_slice();
        let block: Block = Block::decode(block_rlp).unwrap();

        let tx_root = calculate_transaction_root(&block.body.transactions);
        assert_eq!(block.transactions_root, tx_root, "Must be the same");
    }

    #[cfg(not(feature = "optimism"))]
    #[test]
    fn check_receipt_root_optimism() {
        let logs = vec![Log {
            address: Address::ZERO,
            data: LogData::new_unchecked(vec![], Default::default()),
        }];
        let bloom = bloom!("00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000001");
        let receipt = ReceiptWithBloom {
            receipt: Receipt {
                tx_type: TxType::Eip2930,
                success: true,
                cumulative_gas_used: 102068,
                logs,
            },
            bloom,
        };
        let receipt = vec![receipt];
        let root = calculate_receipt_root(&receipt);
        assert_eq!(root, b256!("fe70ae4a136d98944951b2123859698d59ad251a381abc9960fa81cae3d0d4a0"));
    }

    #[test]
    fn check_withdrawals_root() {
        // Single withdrawal, amount 0
        // https://github.com/ethereum/tests/blob/9760400e667eba241265016b02644ef62ab55de2/BlockchainTests/EIPTests/bc4895-withdrawals/amountIs0.json
        let data = &hex!("f90238f90219a0151934ad9b654c50197f37018ee5ee9bb922dec0a1b5e24a6d679cb111cdb107a01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347942adc25665018aa1fe0e6bc666dac8fc2697ff9baa0046119afb1ab36aaa8f66088677ed96cd62762f6d3e65642898e189fbe702d51a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421b90100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008001887fffffffffffffff8082079e42a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b42188000000000000000009a048a703da164234812273ea083e4ec3d09d028300cd325b46a6a75402e5a7ab95c0c0d9d8808094c94f5374fce5edbc8e2a8697c15331677e6ebf0b80");
        let block: Block = Block::decode(&mut data.as_slice()).unwrap();
        assert!(block.body.withdrawals.is_some());
        let withdrawals = block.body.withdrawals.as_ref().unwrap();
        assert_eq!(withdrawals.len(), 1);
        let withdrawals_root = calculate_withdrawals_root(withdrawals);
        assert_eq!(block.withdrawals_root, Some(withdrawals_root));

        // 4 withdrawals, identical indices
        // https://github.com/ethereum/tests/blob/9760400e667eba241265016b02644ef62ab55de2/BlockchainTests/EIPTests/bc4895-withdrawals/twoIdenticalIndex.json
        let data = &hex!("f9028cf90219a0151934ad9b654c50197f37018ee5ee9bb922dec0a1b5e24a6d679cb111cdb107a01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347942adc25665018aa1fe0e6bc666dac8fc2697ff9baa0ccf7b62d616c2ad7af862d67b9dcd2119a90cebbff8c3cd1e5d7fc99f8755774a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421b90100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008001887fffffffffffffff8082079e42a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b42188000000000000000009a0a95b9a7b58a6b3cb4001eb0be67951c5517141cb0183a255b5cae027a7b10b36c0c0f86cda808094c94f5374fce5edbc8e2a8697c15331677e6ebf0b822710da028094c94f5374fce5edbc8e2a8697c15331677e6ebf0b822710da018094c94f5374fce5edbc8e2a8697c15331677e6ebf0b822710da028094c94f5374fce5edbc8e2a8697c15331677e6ebf0b822710");
        let block: Block = Block::decode(&mut data.as_slice()).unwrap();
        assert!(block.body.withdrawals.is_some());
        let withdrawals = block.body.withdrawals.as_ref().unwrap();
        assert_eq!(withdrawals.len(), 4);
        let withdrawals_root = calculate_withdrawals_root(withdrawals);
        assert_eq!(block.withdrawals_root, Some(withdrawals_root));
    }

    #[test]
    fn check_empty_state_root() {
        let genesis_alloc = HashMap::<Address, GenesisAccount>::new();
        let root = state_root_unhashed(genesis_alloc);
        assert_eq!(root, EMPTY_ROOT_HASH);
    }

    #[test]
    fn test_simple_account_state_root() {
        // each fixture specifies an address and expected root hash - the address is initialized
        // with a maximum balance, and is the only account in the state.
        // these test cases are generated by using geth with a custom genesis.json (with a single
        // account that has max balance)
        let fixtures: Vec<(Address, B256)> = vec![
            (
                hex!("9fe4abd71ad081f091bd06dd1c16f7e92927561e").into(),
                hex!("4b35be4231841d212ce2fa43aedbddeadd6eb7d420195664f9f0d55629db8c32").into(),
            ),
            (
                hex!("c2ba9d87f8be0ade00c60d3656c1188e008fbfa2").into(),
                hex!("e1389256c47d63df8856d7729dec9dc2dae074a7f0cbc49acad1cf7b29f7fe94").into(),
            ),
        ];

        for (test_addr, expected_root) in fixtures {
            let mut genesis_alloc = HashMap::new();
            genesis_alloc
                .insert(test_addr, GenesisAccount { balance: U256::MAX, ..Default::default() });

            let root = state_root_unhashed(genesis_alloc);

            assert_eq!(root, expected_root);
        }
    }

    #[test]
    fn test_chain_state_roots() {
        let expected_mainnet_state_root =
            b256!("d7f8974fb5ac78d9ac099b9ad5018bedc2ce0a72dad1827a1709da30580f0544");
        let calculated_mainnet_state_root = state_root_ref_unhashed(&MAINNET.genesis.alloc);
        assert_eq!(
            expected_mainnet_state_root, calculated_mainnet_state_root,
            "mainnet state root mismatch"
        );

        let expected_sepolia_state_root =
            b256!("5eb6e371a698b8d68f665192350ffcecbbbf322916f4b51bd79bb6887da3f494");
        let calculated_sepolia_state_root = state_root_ref_unhashed(&SEPOLIA.genesis.alloc);
        assert_eq!(
            expected_sepolia_state_root, calculated_sepolia_state_root,
            "sepolia state root mismatch"
        );

        let expected_holesky_state_root =
            b256!("69d8c9d72f6fa4ad42d4702b433707212f90db395eb54dc20bc85de253788783");
        let calculated_holesky_state_root = state_root_ref_unhashed(&HOLESKY.genesis.alloc);
        assert_eq!(
            expected_holesky_state_root, calculated_holesky_state_root,
            "holesky state root mismatch"
        );
    }
}
