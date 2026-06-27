use alloy_eip8289::{WamItem, WarmAccessMultiset};
use alloy_evm::env::BlockEnvironment;
use revm::{
    context_interface::block::WarmAccessList,
    primitives::{AddressMap, HashSet, StorageKey},
};

/// Warm accesses inherited from the current EIP-8289 warming window.
#[derive(Clone, Debug, Default)]
pub(crate) struct WarmAccessSnapshot {
    items: WarmAccessList,
}

impl WarmAccessSnapshot {
    pub(crate) fn from_multiset(warm_accesses: &WarmAccessMultiset) -> Self {
        let mut items: AddressMap<HashSet<StorageKey>> = AddressMap::default();

        for (item, count) in warm_accesses.iter() {
            if count == 0 {
                continue;
            }

            match item {
                WamItem::Account(address) => {
                    items.entry(*address).or_default();
                }
                WamItem::Slot { address, key } => {
                    items.entry(*address).or_default().insert(*key);
                }
            }
        }

        Self {
            items: items
                .into_iter()
                .map(|(address, keys)| (address, keys.into_iter().collect()))
                .collect(),
        }
    }

    pub(crate) fn apply_to_block_env(&self, block_env: &mut impl BlockEnvironment) {
        block_env.inner_mut().warm_accesses = self.items.clone();
    }
}
