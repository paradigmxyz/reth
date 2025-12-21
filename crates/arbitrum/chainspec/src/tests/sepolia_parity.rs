#[cfg(test)]
mod tests {
    use super::super::*;
    use alloy_primitives::{b256, B256};
    use reth_chainspec::ChainSpec;
    use reth_trie_common::root::state_root;

    use crate::embedded_alloc::load_sepolia_secure_alloc_hashed;

    #[test]
    fn baked_genesis_builds_with_embedded_alloc_or_fallback() {
        let spec = sepolia_baked_genesis_from_header(
            421_614,
            "0x5f5e100",  // base_fee
            "0x0",        // timestamp
            "0x8647a2ae10b316ca12fbd76327fe4d64d12cb0ec664a128b0d59df15d05391be",  // state_root
            "0x1c9c380",  // gas_limit
            "0x",         // extra_data
            "0x0000000000000000000000000000000000000000000000000000000000000000",  // mix_hash
            "0x0",        // nonce
            None,         // chain_config_bytes
            Some("0xb2d05e00"),  // initial_l1_base_fee
        )
        .expect("chainspec");
        let hash = spec.genesis_hash();
        assert_eq!(
            hash,
            b256!("0x77194da4010e549a7028a9c3c51c3e277823be6ac7d138d0bb8a70197b5c004c")
        );
    }

    #[test]
    fn sepolia_securealloc_trie_root_matches() {
        let (accounts_h, storages_h) = load_sepolia_secure_alloc_hashed().expect("load");
        let root = state_root(&accounts_h, &storages_h);
        assert_eq!(
            root,
            b256!("0x8647a2ae10b316ca12fbd76327fe4d64d12cb0ec664a128b0d59df15d05391be")
        );
    }

    #[test]
    fn check_arbos_address_in_alloc() {
        use alloy_primitives::{keccak256, Address, address};

        let (accounts_h, _) = load_sepolia_secure_alloc_hashed().expect("load");

        // ArbosAddress = 0xa4b05
        let arbos_addr = address!("00000000000000000000000000000000000a4b05");
        let hashed = keccak256(arbos_addr);

        println!("ArbosAddress: {:?}", arbos_addr);
        println!("Hashed: {:?}", hashed);
        println!("Total accounts in alloc: {}", accounts_h.len());

        if let Some(acct) = accounts_h.get(&hashed) {
            println!("Found ArbosAddress in alloc: nonce={}, code_hash={:?}", acct.nonce, acct.bytecode_hash);
            // Check if it has the 0xfe code (keccak256(0xfe) = a specific hash)
            let fe_code_hash = keccak256(&[0xfe]);
            println!("Expected code hash for 0xfe: {:?}", fe_code_hash);
            assert_eq!(acct.bytecode_hash, Some(fe_code_hash), "ArbosAddress should have 0xfe code");
        } else {
            println!("ArbosAddress NOT found in alloc!");
            println!("Listing all hashed addresses:");
            for (h, a) in accounts_h.iter() {
                println!("  {:?}: nonce={} code_hash={:?}", h, a.nonce, a.bytecode_hash);
            }
            panic!("ArbosAddress (0xa4b05) not found in embedded alloc");
        }
    }

    #[test]
    fn check_arbos_storage_backing_in_alloc() {
        use alloy_primitives::{keccak256, Address, address};

        let (accounts_h, _) = load_sepolia_secure_alloc_hashed().expect("load");

        // ArbOS storage backing = 0xA4B05FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF
        let arbos_storage_addr = address!("A4B05FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF");
        let hashed = keccak256(arbos_storage_addr);

        println!("ArbOS Storage Backing: {:?}", arbos_storage_addr);
        println!("Hashed: {:?}", hashed);
        println!("Total accounts in alloc: {}", accounts_h.len());

        println!("\nAll accounts in alloc:");
        for (h, a) in accounts_h.iter() {
            println!("  {:?}: nonce={} balance={} code_hash={:?}", h, a.nonce, a.balance, a.bytecode_hash);
        }

        if let Some(acct) = accounts_h.get(&hashed) {
            println!("\nFound ArbOS Storage Backing in alloc: nonce={}, balance={}", acct.nonce, acct.balance);
            assert_eq!(acct.nonce, 1, "ArbOS storage backing should have nonce=1");
        } else {
            println!("\nArbOS Storage Backing NOT found in alloc!");
            panic!("ArbOS Storage Backing (0xA4B05FFF...) not found in embedded alloc");
        }
    }
}
