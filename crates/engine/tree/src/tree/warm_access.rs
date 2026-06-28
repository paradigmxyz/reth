use alloy_eip7928::bal::DecodedBal;
use alloy_eip8289::{WamItem, WamItems, WarmAccessMultiset};
use alloy_evm::env::BlockEnvironment;
use revm::{
    context_interface::block::WarmAccessList,
    primitives::{AddressMap, HashSet, StorageKey},
};

/// Current-block accesses that are already present in the EIP-8289 warming window.
#[derive(Clone, Debug, Default)]
pub(crate) struct WarmAccessSnapshot {
    items: Option<WarmAccessList>,
}

impl WarmAccessSnapshot {
    pub(crate) fn from_wam_and_bal(
        warm_accesses: &WarmAccessMultiset,
        decoded_bal: Option<&DecodedBal>,
    ) -> Self {
        let mut items: AddressMap<HashSet<StorageKey>> = AddressMap::default();

        let Some(decoded_bal) = decoded_bal else {
            return Self::default();
        };

        for item in WamItems::from_accounts(decoded_bal.as_bal().as_slice()) {
            if !warm_accesses.is_warm(&item) {
                continue;
            }

            match item {
                WamItem::Account(address) => {
                    items.entry(address).or_default();
                }
                WamItem::Slot { address, key } => {
                    items.entry(address).or_default().insert(key);
                }
            }
        }

        Self {
            items: Some(
                items
                    .into_iter()
                    .map(|(address, keys)| (address, keys.into_iter().collect()))
                    .collect(),
            ),
        }
    }

    pub(crate) fn apply_to_block_env(&self, block_env: &mut impl BlockEnvironment) {
        block_env.inner_mut().warm_accesses = self.items.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eip7928::{bal::Bal, AccountChanges};
    use alloy_primitives::{address, Bytes, U256};
    use revm::context::BlockEnv;

    #[test]
    fn warm_snapshot_only_contains_payload_bal_items_present_in_wam() {
        let warm_address = address!("0000000000000000000000000000000000000001");
        let cold_address = address!("0000000000000000000000000000000000000002");
        let wam_only_address = address!("0000000000000000000000000000000000000003");

        let mut warm_accesses = WarmAccessMultiset::new();
        let wam_items = WamItems::new(vec![
            WamItem::account(warm_address),
            WamItem::slot(warm_address, U256::from(1)),
            WamItem::account(wam_only_address),
            WamItem::slot(wam_only_address, U256::from(9)),
        ]);
        warm_accesses.apply_item_transition(&wam_items, None);

        let decoded_bal = DecodedBal::new(
            Bal::new(vec![
                AccountChanges::new(warm_address)
                    .with_storage_read(U256::from(1))
                    .with_storage_read(U256::from(2)),
                AccountChanges::new(cold_address).with_storage_read(U256::from(1)),
            ]),
            Bytes::new(),
        );

        let snapshot = WarmAccessSnapshot::from_wam_and_bal(&warm_accesses, Some(&decoded_bal));

        let items = snapshot.items.as_ref().unwrap();
        assert_eq!(items.len(), 1);
        let (address, slots) = &items[0];
        assert_eq!(*address, warm_address);
        assert_eq!(slots.as_slice(), &[U256::from(1)]);
    }

    #[test]
    fn warm_snapshot_is_empty_without_payload_bal() {
        let mut warm_accesses = WarmAccessMultiset::new();
        let wam_items = WamItems::new(vec![WamItem::account(address!(
            "0000000000000000000000000000000000000001"
        ))]);
        warm_accesses.apply_item_transition(&wam_items, None);

        let snapshot = WarmAccessSnapshot::from_wam_and_bal(&warm_accesses, None);

        assert!(snapshot.items.is_none());
    }

    #[test]
    fn default_warm_snapshot_applies_no_warm_accesses_to_block_env() {
        let mut block_env = BlockEnv::default();

        WarmAccessSnapshot::default().apply_to_block_env(&mut block_env);

        assert!(block_env.warm_accesses.is_none());
    }

    #[test]
    fn warm_snapshot_applies_warm_accesses_to_block_env() {
        let warm_address = address!("0000000000000000000000000000000000000001");

        let mut warm_accesses = WarmAccessMultiset::new();
        warm_accesses.apply_item_transition(
            &WamItems::new(vec![WamItem::slot(warm_address, U256::from(1))]),
            None,
        );
        let decoded_bal = DecodedBal::new(
            Bal::new(vec![AccountChanges::new(warm_address).with_storage_read(U256::from(1))]),
            Bytes::new(),
        );
        let snapshot = WarmAccessSnapshot::from_wam_and_bal(&warm_accesses, Some(&decoded_bal));
        let mut block_env = BlockEnv::default();

        snapshot.apply_to_block_env(&mut block_env);

        let applied = block_env.warm_accesses.as_ref().unwrap();
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].0, warm_address);
        assert_eq!(applied[0].1.as_slice(), &[U256::from(1)]);
    }
}
