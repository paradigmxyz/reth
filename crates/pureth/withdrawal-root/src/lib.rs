#![allow(missing_docs, rustdoc::missing_crate_level_docs)]

use alloy_eips::eip4895::Withdrawal;
use alloy_primitives::{b256, B256};
use sha2::{Digest, Sha256};
use tree_hash::{merkle_root, mix_in_length, TreeHash};

const WITHDRAWAL_ACTIVE_FIELDS: B256 =
    b256!("0x0f00000000000000000000000000000000000000000000000000000000000000");

pub fn progressive_withdrawals_root(withdrawals: &[Withdrawal]) -> B256 {
    let roots = withdrawals.iter().map(withdrawal_root).collect::<Vec<_>>();
    mix_in_length(&progressive_root(&roots), withdrawals.len())
}

fn withdrawal_root(withdrawal: &Withdrawal) -> B256 {
    let fields = [
        withdrawal.index.tree_hash_root(),
        withdrawal.validator_index.tree_hash_root(),
        withdrawal.address.tree_hash_root(),
        withdrawal.amount.tree_hash_root(),
    ];
    hash_pair(progressive_root(&fields), WITHDRAWAL_ACTIVE_FIELDS)
}

fn progressive_root(roots: &[B256]) -> B256 {
    fn recurse(roots: &[B256], group_size: usize) -> B256 {
        if roots.is_empty() {
            return B256::ZERO;
        }

        let split = roots.len().min(group_size);
        let bytes = roots[..split].iter().flat_map(|root| root.iter().copied()).collect::<Vec<_>>();
        let left = merkle_root(&bytes, group_size);
        let right = if split == roots.len() {
            B256::ZERO
        } else {
            let next_group_size =
                group_size.checked_mul(4).expect("progressive group size overflow");
            recurse(&roots[split..], next_group_size)
        };
        hash_pair(left, right)
    }

    recurse(roots, 1)
}

fn hash_pair(left: B256, right: B256) -> B256 {
    let mut bytes = [0; 64];
    bytes[..32].copy_from_slice(left.as_slice());
    bytes[32..].copy_from_slice(right.as_slice());
    B256::from(<[u8; 32]>::from(Sha256::digest(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::Address;

    fn withdrawal_a() -> Withdrawal {
        Withdrawal { index: 1, validator_index: 2, address: Address::repeat_byte(0x11), amount: 3 }
    }

    fn withdrawal_b() -> Withdrawal {
        Withdrawal { index: 4, validator_index: 5, address: Address::repeat_byte(0x22), amount: 6 }
    }

    fn synthetic_roots(count: usize) -> Vec<B256> {
        (0..count).map(|index| B256::repeat_byte((index + 1) as u8)).collect()
    }

    #[test]
    fn withdrawal_container_root_matches_reference() {
        assert_eq!(
            withdrawal_root(&withdrawal_a()),
            b256!("0x2b5b26f1066bc03633d17dad23e88ba2cbdcc605ce63d98bb62ffc8daf6e91cf")
        );
    }

    #[test]
    fn empty_and_singleton_roots_match_reference() {
        assert_eq!(
            progressive_withdrawals_root(&[]),
            b256!("0xf5a5fd42d16a20302798ef6ed309979b43003d2320d9f0e8ea9831a92759fb4b")
        );
        assert_eq!(
            progressive_withdrawals_root(&[withdrawal_a()]),
            b256!("0x48cd06fcb026799de708515f04feaf9a67753755627c208dddbc4b7739348542")
        );
    }

    #[test]
    fn value_and_order_roots_match_reference() {
        let a = withdrawal_a();
        let b = withdrawal_b();
        let mutated = Withdrawal { amount: 4, ..a };

        assert_eq!(
            progressive_withdrawals_root(&[mutated]),
            b256!("0x98085deb6869c7070c58427174a003aa8f37040866062a779027cc4aaa26ddcf")
        );
        assert_eq!(
            progressive_withdrawals_root(&[a, b]),
            b256!("0x3f062776a629a4ac834b9c28c068375223340848f02368cf852215e937dad7e9")
        );
        assert_eq!(
            progressive_withdrawals_root(&[b, a]),
            b256!("0x00dfbb0f5cffdb7408edd6fb371e7503beb55b7f71a267c759e6805472fff887")
        );
    }

    #[test]
    fn progressive_group_boundary_roots_match_reference() {
        assert_eq!(
            mix_in_length(&progressive_root(&synthetic_roots(5)), 5),
            b256!("0x183886e81b2e887d5960b2fa49b3464eabee62ec55ff5e6ee6f7e0495d8a01d1")
        );
        assert_eq!(
            mix_in_length(&progressive_root(&synthetic_roots(6)), 6),
            b256!("0x690beb7f075e2dc91699aa3ee9354687772923889ce755458cb405ae95e34055")
        );
    }

    #[test]
    fn ssz_specs_v0_1_0_progressive_list_vectors_match() {
        assert_eq!(
            mix_in_length(&progressive_root(&[]), 0),
            b256!("0xf5a5fd42d16a20302798ef6ed309979b43003d2320d9f0e8ea9831a92759fb4b")
        );

        let mut singleton = [0; 32];
        singleton[..8].copy_from_slice(&1_u64.to_le_bytes());
        assert_eq!(
            mix_in_length(&progressive_root(&[B256::from(singleton)]), 1),
            b256!("0x905efb51c2764c2c7a4efb0548e372569df06db82115c3b1896c186632f3fe5b")
        );

        let mut chunks = [[0; 32]; 6];
        for value in 0_u64..21 {
            let chunk = &mut chunks[value as usize / 4];
            let offset = value as usize % 4 * 8;
            chunk[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        }
        assert_eq!(
            mix_in_length(&progressive_root(&chunks.map(B256::from)), 21),
            b256!("0x86a8ce9749021379ba7af31ac5d6f3b33e0e0791a5c9bb2e00580fe8d6aeb117")
        );
    }

    #[test]
    fn ssz_specs_v0_1_0_progressive_container_vector_matches() {
        let mut field = [0; 32];
        field[..2].copy_from_slice(&48_879_u16.to_le_bytes());

        let mut active_fields = [0; 32];
        active_fields[0] = 1;

        assert_eq!(
            hash_pair(progressive_root(&[B256::from(field)]), B256::from(active_fields)),
            b256!("0xa88f083e786a9c55bf466e4954c4e25b3112cad5a5a2f85248d3a88d42b9fc58")
        );
    }

    #[test]
    fn withdrawal_fields_use_ssz_encoding() {
        let withdrawal = Withdrawal {
            index: 0x0102_0304_0506_0708,
            validator_index: 0x1122_3344_5566_7788,
            amount: u64::MAX,
            ..withdrawal_a()
        };

        let mut index = [0; 32];
        index[..8].copy_from_slice(&withdrawal.index.to_le_bytes());
        assert_eq!(withdrawal.index.tree_hash_root(), B256::from(index));

        let mut validator_index = [0; 32];
        validator_index[..8].copy_from_slice(&withdrawal.validator_index.to_le_bytes());
        assert_eq!(withdrawal.validator_index.tree_hash_root(), B256::from(validator_index));

        let mut amount = [0; 32];
        amount[..8].copy_from_slice(&withdrawal.amount.to_le_bytes());
        assert_eq!(withdrawal.amount.tree_hash_root(), B256::from(amount));

        let mut address = [0; 32];
        address[..20].fill(0x11);
        assert_eq!(withdrawal.address.tree_hash_root(), B256::from(address));

        assert_eq!(
            withdrawal_root(&withdrawal),
            b256!("0x9e37c11c2397a10b17a16ab7ef8b02dc7c8d4645d704f4b326b9327e42cc7727")
        );
    }

    #[test]
    fn progressive_larger_group_boundary_roots_match_reference() {
        assert_eq!(
            mix_in_length(&progressive_root(&synthetic_roots(21)), 21),
            b256!("0x93589633f10a1e8fe51bef0481731c7c19d7a87269127b6c6a19720668ee47da")
        );
        assert_eq!(
            mix_in_length(&progressive_root(&synthetic_roots(22)), 22),
            b256!("0xe79bdcda4e58dd09c4b855964e1f1c01c99e215b6e01602f5302763effaf8637")
        );
    }
}
