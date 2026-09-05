#![allow(missing_docs, rustdoc::missing_crate_level_docs)]

use std::fmt;

use alloy_eips::eip4895::Withdrawal;
use alloy_primitives::B256;
use tree_hash::{merkle_root, mix_in_length, TreeHash};

const MAX_WITHDRAWALS_PER_PAYLOAD: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TooManyWithdrawals(usize);

impl fmt::Display for TooManyWithdrawals {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "payload contains {} withdrawals; maximum is {MAX_WITHDRAWALS_PER_PAYLOAD}",
            self.0
        )
    }
}

impl std::error::Error for TooManyWithdrawals {}

pub fn progressive_withdrawals_root(
    withdrawals: &[Withdrawal],
) -> Result<B256, TooManyWithdrawals> {
    if withdrawals.len() > MAX_WITHDRAWALS_PER_PAYLOAD {
        return Err(TooManyWithdrawals(withdrawals.len()));
    }

    let roots = withdrawals.iter().map(withdrawal_root).collect::<Vec<_>>();
    Ok(mix_in_length(&progressive_root(&roots, 1), withdrawals.len()))
}

fn withdrawal_root(withdrawal: &Withdrawal) -> B256 {
    let fields = [
        withdrawal.index.tree_hash_root(),
        withdrawal.validator_index.tree_hash_root(),
        withdrawal.address.tree_hash_root(),
        withdrawal.amount.tree_hash_root(),
    ];
    let mut active_fields = [0; 32];
    active_fields[0] = 0x0f;
    hash_pair(progressive_root(&fields, 1), B256::from(active_fields))
}

fn progressive_root(roots: &[B256], group_size: usize) -> B256 {
    if roots.is_empty() {
        return B256::ZERO;
    }

    let split = roots.len().min(group_size);
    let bytes = roots[..split].iter().flat_map(|root| root.iter().copied()).collect::<Vec<_>>();
    let left = merkle_root(&bytes, group_size);
    let right = progressive_root(&roots[split..], group_size * 4);
    hash_pair(left, right)
}

fn hash_pair(left: B256, right: B256) -> B256 {
    let mut bytes = [0; 64];
    bytes[..32].copy_from_slice(left.as_slice());
    bytes[32..].copy_from_slice(right.as_slice());
    merkle_root(&bytes, 2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{b256, Address};

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
            progressive_withdrawals_root(&[]).unwrap(),
            b256!("0xf5a5fd42d16a20302798ef6ed309979b43003d2320d9f0e8ea9831a92759fb4b")
        );
        assert_eq!(
            progressive_withdrawals_root(&[withdrawal_a()]).unwrap(),
            b256!("0x48cd06fcb026799de708515f04feaf9a67753755627c208dddbc4b7739348542")
        );
    }

    #[test]
    fn value_and_order_roots_match_reference() {
        let a = withdrawal_a();
        let b = withdrawal_b();
        let mutated = Withdrawal { amount: 4, ..a };

        assert_eq!(
            progressive_withdrawals_root(&[mutated]).unwrap(),
            b256!("0x98085deb6869c7070c58427174a003aa8f37040866062a779027cc4aaa26ddcf")
        );
        assert_eq!(
            progressive_withdrawals_root(&[a, b]).unwrap(),
            b256!("0x3f062776a629a4ac834b9c28c068375223340848f02368cf852215e937dad7e9")
        );
        assert_eq!(
            progressive_withdrawals_root(&[b, a]).unwrap(),
            b256!("0x00dfbb0f5cffdb7408edd6fb371e7503beb55b7f71a267c759e6805472fff887")
        );
    }

    #[test]
    fn progressive_group_boundary_roots_match_reference() {
        assert_eq!(
            mix_in_length(&progressive_root(&synthetic_roots(5), 1), 5),
            b256!("0x183886e81b2e887d5960b2fa49b3464eabee62ec55ff5e6ee6f7e0495d8a01d1")
        );
        assert_eq!(
            mix_in_length(&progressive_root(&synthetic_roots(6), 1), 6),
            b256!("0x690beb7f075e2dc91699aa3ee9354687772923889ce755458cb405ae95e34055")
        );
    }

    #[test]
    fn withdrawal_fields_use_ssz_encoding() {
        let withdrawal = Withdrawal { index: 0x0102_0304_0506_0708, ..withdrawal_a() };

        let mut index = [0; 32];
        index[..8].copy_from_slice(&withdrawal.index.to_le_bytes());
        assert_eq!(withdrawal.index.tree_hash_root(), B256::from(index));

        let mut address = [0; 32];
        address[..20].fill(0x11);
        assert_eq!(withdrawal.address.tree_hash_root(), B256::from(address));
    }

    #[test]
    fn payload_limit_is_enforced() {
        assert!(progressive_withdrawals_root(&vec![withdrawal_a(); MAX_WITHDRAWALS_PER_PAYLOAD])
            .is_ok());

        let withdrawals = vec![withdrawal_a(); MAX_WITHDRAWALS_PER_PAYLOAD + 1];
        assert_eq!(
            progressive_withdrawals_root(&withdrawals),
            Err(TooManyWithdrawals(MAX_WITHDRAWALS_PER_PAYLOAD + 1))
        );
    }
}
