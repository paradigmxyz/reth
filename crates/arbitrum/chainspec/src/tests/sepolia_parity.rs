#[cfg(test)]
mod tests {
    use super::super::*;
    use alloy_primitives::{b256, B256};
    use reth_chainspec::ChainSpec;
    use reth_trie_common::root::state_root;

    use crate::embedded_alloc::load_sepolia_secure_alloc_hashed;

    #[ignore]
    #[test]
    fn baked_genesis_builds_with_embedded_alloc_or_fallback() {
        let spec = sepolia_baked_genesis_from_header(
            421_614,
            "0x5f5e100",
            "0x0",
            "0x8647a2ae10b316ca12fbd76327fe4d64d12cb0ec664a128b0d59df15d05391be",
            "0x1c9c380",
            "0x",
            None,
            Some("0xb2d05e00"),
        )
        .expect("chainspec");
        let _hash = spec.genesis_hash();
    }

    #[ignore]
    #[test]
    fn sepolia_securealloc_trie_root_matches() {
        let (accounts_h, storages_h) = load_sepolia_secure_alloc_hashed().expect("load");
        let root = state_root(&accounts_h, &storages_h);
        assert_eq!(
            root,
            b256!("0x8647a2ae10b316ca12fbd76327fe4d64d12cb0ec664a128b0d59df15d05391be")
        );
    }
}
