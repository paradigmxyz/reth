//! Structural BAL validation: the two cheapest checks in the validation chain.
//!
//! Both run before any state I/O, letting us reject adversarial BALs without spending storage
//! bandwidth. See `BAL.md` §Validation chain, checks A and B.

use alloy_eip7928::{bal::DecodedBal, total_bal_items, AccountChanges, ITEM_COST};
use alloy_primitives::B256;

use super::RejectReason;

/// Check A: `keccak256(raw_rlp_of_received_bal) == header.block_access_list_hash`.
///
/// Uses [`DecodedBal`]'s cached hash (`keccak256` of the raw RLP bytes captured at decode
/// time) — the same pre-image the header commits to. Zero I/O; first call computes the hash,
/// subsequent calls are free.
pub fn check_bal_hash(bal: &DecodedBal, expected: B256) -> Result<(), RejectReason> {
    let computed = bal.hash();
    if computed == expected {
        Ok(())
    } else {
        Err(RejectReason::HeaderHashMismatch { computed, expected })
    }
}

/// Check B: `(addresses + unique_storage_keys) * ITEM_COST <= block_gas_limit`.
///
/// `unique_storage_keys` dedupes `storage_reads ∪ storage_changes` per account (delegated to
/// `alloy_eip7928::total_bal_items`). Saturating arithmetic defends against adversarial BALs
/// whose raw item count would overflow `u64` — they reject cleanly instead of panicking.
pub fn check_item_count(bal: &[AccountChanges], block_gas_limit: u64) -> Result<(), RejectReason> {
    // TODO: use `Bal::validate_gas_limit` once the pinned alloy-eip7928 release exposes it.
    let bal_items = total_bal_items(bal);
    let total_cost = bal_items.saturating_mul(ITEM_COST as u64);
    if total_cost <= block_gas_limit {
        Ok(())
    } else {
        Err(RejectReason::ItemCountExceedsGasBudget { bal_items, gas_limit: block_gas_limit })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eip7928::{bal::Bal as AlloyBal, AccountChanges, SlotChanges, StorageChange};
    use alloy_primitives::{b256, Address, U256};

    fn addr(byte: u8) -> Address {
        let mut a = [0u8; 20];
        a[19] = byte;
        Address::from(a)
    }

    fn slot(byte: u8) -> U256 {
        U256::from(byte)
    }

    /// Wraps a `Vec<AccountChanges>` into a `DecodedBal` by RLP-encoding the contents — same
    /// path a real block takes through `DecodedBal::from_rlp_bytes`, so the cached hash stays
    /// faithful to what the header would commit to.
    fn to_decoded(bal: Vec<AccountChanges>) -> DecodedBal {
        let alloy_bal: AlloyBal = bal.into();
        let raw = alloy_rlp::encode(&alloy_bal).into();
        DecodedBal::new(alloy_bal, raw)
    }

    // ---- check_bal_hash ----

    #[test]
    fn check_bal_hash_accepts_matching_hash() {
        let decoded = to_decoded(vec![AccountChanges::new(addr(1))]);
        let computed = decoded.hash();
        assert_eq!(check_bal_hash(&decoded, computed), Ok(()));
    }

    #[test]
    fn check_bal_hash_rejects_with_populated_fields() {
        let decoded = to_decoded(vec![AccountChanges::new(addr(1))]);
        let expected = b256!("0xdeadbeef00000000000000000000000000000000000000000000000000000000");

        let err = check_bal_hash(&decoded, expected).unwrap_err();

        match err {
            RejectReason::HeaderHashMismatch { computed, expected: got } => {
                assert_eq!(got, expected);
                assert_eq!(computed, decoded.hash());
                assert_ne!(computed, expected);
            }
            other => panic!("expected HeaderHashMismatch, got {other:?}"),
        }
    }

    #[test]
    fn check_bal_hash_handles_empty_bal() {
        let decoded = to_decoded(Vec::new());
        let computed = decoded.hash();
        assert_eq!(check_bal_hash(&decoded, computed), Ok(()));
    }

    // ---- check_item_count ----

    #[test]
    fn check_item_count_accepts_empty_bal_with_zero_limit() {
        assert_eq!(check_item_count(&[], 0), Ok(()));
    }

    #[test]
    fn check_item_count_counts_addresses_and_union_of_slots() {
        // 2 addresses, disjoint read/write slots totalling 4 unique keys → 6 items.
        let bal = vec![
            AccountChanges {
                address: addr(1),
                storage_reads: vec![slot(10), slot(11)],
                storage_changes: vec![SlotChanges::new(
                    slot(20),
                    vec![StorageChange::new(1, U256::from(1))],
                )],
                ..Default::default()
            },
            AccountChanges {
                address: addr(2),
                storage_changes: vec![SlotChanges::new(
                    slot(30),
                    vec![StorageChange::new(2, U256::from(2))],
                )],
                ..Default::default()
            },
        ];

        // 6 items * 2000 = 12_000. Limit exactly at cost → Ok (spec is `<=`).
        assert_eq!(check_item_count(&bal, 12_000), Ok(()));

        // One gas under → rejects with the raw item count in the error.
        let err = check_item_count(&bal, 11_999).unwrap_err();
        assert_eq!(
            err,
            RejectReason::ItemCountExceedsGasBudget { bal_items: 6, gas_limit: 11_999 }
        );
    }

    #[test]
    fn check_item_count_dedups_slot_appearing_in_reads_and_changes() {
        // Malformed BAL: same slot listed in both storage_reads and storage_changes.
        // total_bal_items dedups, so this counts as 1 address + 1 slot = 2 items.
        let bal = vec![AccountChanges {
            address: addr(1),
            storage_reads: vec![slot(10)],
            storage_changes: vec![SlotChanges::new(
                slot(10),
                vec![StorageChange::new(1, U256::from(1))],
            )],
            ..Default::default()
        }];

        assert_eq!(check_item_count(&bal, 4_000), Ok(()));
        let err = check_item_count(&bal, 3_999).unwrap_err();
        assert!(matches!(err, RejectReason::ItemCountExceedsGasBudget { bal_items: 2, .. }));
    }

    #[test]
    fn check_item_count_rejects_tightly_over_budget() {
        let bal = vec![AccountChanges::new(addr(1))]; // 1 item.
        assert_eq!(check_item_count(&bal, 2_000), Ok(()));
        let err = check_item_count(&bal, 1_999).unwrap_err();
        assert_eq!(err, RejectReason::ItemCountExceedsGasBudget { bal_items: 1, gas_limit: 1_999 });
    }

    #[test]
    fn check_item_count_tolerates_near_u64_item_count() {
        // We can't actually build a u64::MAX-sized Vec, but we can verify the arithmetic helper
        // is saturating by confirming that for any realistic count the cost comparison is
        // well-defined (no panic). This test is a smoke test for the saturating_mul path.
        let bal: Vec<AccountChanges> = (0u8..=200).map(|i| AccountChanges::new(addr(i))).collect();
        // 201 items * 2000 = 402_000, well inside u64 and under any realistic gas limit.
        assert!(check_item_count(&bal, 402_000).is_ok());
    }

    #[test]
    fn item_cost_constant_matches_spec() {
        // EIP-7928 + BAL.md §Validation chain pin this at 2000.
        assert_eq!(ITEM_COST, 2000);
    }
}
