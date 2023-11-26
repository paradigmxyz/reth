//! Helper function for calculating Merkle proofs and hashes.

use alloy_rlp::Encodable;
use itertools::Itertools;
use reth_primitives::{
    keccak256,
    trie::{HashBuilder, Nibbles},
    Address, GenesisAccount, B256,
};
use std::collections::HashMap;

/// Calculates the root hash for the state, this corresponds to [geth's
/// `deriveHash`](https://github.com/ethereum/go-ethereum/blob/6c149fd4ad063f7c24d726a73bc0546badd1bc73/core/genesis.go#L119).
pub fn genesis_state_root(genesis_alloc: &HashMap<Address, GenesisAccount>) -> B256 {
    let accounts_with_sorted_hashed_keys = genesis_alloc
        .iter()
        .map(|(address, account)| (keccak256(address), account))
        .sorted_by_key(|(key, _)| *key);

    let mut hb = HashBuilder::default();
    let mut account_rlp_buf = Vec::new();
    for (hashed_key, account) in accounts_with_sorted_hashed_keys {
        account_rlp_buf.clear();
        account.encode(&mut account_rlp_buf);
        hb.add_leaf(Nibbles::unpack(hashed_key), &account_rlp_buf);
    }

    hb.root()
}

#[cfg(test)]
mod tests {
    use crate::{GOERLI, HOLESKY, MAINNET, SEPOLIA};
    use alloy_primitives::b256;
    use reth_primitives::proofs::genesis_state_root;

    #[test]
    fn test_chain_state_roots() {
        let expected_mainnet_state_root =
            b256!("d7f8974fb5ac78d9ac099b9ad5018bedc2ce0a72dad1827a1709da30580f0544");
        let calculated_mainnet_state_root = genesis_state_root(&MAINNET.genesis.alloc);
        assert_eq!(
            expected_mainnet_state_root, calculated_mainnet_state_root,
            "mainnet state root mismatch"
        );

        let expected_goerli_state_root =
            b256!("5d6cded585e73c4e322c30c2f782a336316f17dd85a4863b9d838d2d4b8b3008");
        let calculated_goerli_state_root = genesis_state_root(&GOERLI.genesis.alloc);
        assert_eq!(
            expected_goerli_state_root, calculated_goerli_state_root,
            "goerli state root mismatch"
        );

        let expected_sepolia_state_root =
            b256!("5eb6e371a698b8d68f665192350ffcecbbbf322916f4b51bd79bb6887da3f494");
        let calculated_sepolia_state_root = genesis_state_root(&SEPOLIA.genesis.alloc);
        assert_eq!(
            expected_sepolia_state_root, calculated_sepolia_state_root,
            "sepolia state root mismatch"
        );

        let expected_holesky_state_root =
            b256!("69d8c9d72f6fa4ad42d4702b433707212f90db395eb54dc20bc85de253788783");
        let calculated_holesky_state_root = genesis_state_root(&HOLESKY.genesis.alloc);
        assert_eq!(
            expected_holesky_state_root, calculated_holesky_state_root,
            "holesky state root mismatch"
        );
    }
}
