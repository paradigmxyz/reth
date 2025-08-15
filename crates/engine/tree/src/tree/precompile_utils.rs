//! Utilities for working with precompiles

use alloy_evm::precompiles::{DynPrecompile, PrecompilesMap};
use alloy_primitives::Address;

/// Extension trait for PrecompilesMap to add pure precompile mapping functionality
pub trait PrecompileMapExt {
    /// Maps only pure precompiles (addresses 0x01-0x0a) with the given closure.
    ///
    /// Pure precompiles are those that don't have side effects or depend on contract state,
    /// and are safe to cache. These are the standard Ethereum precompiles at addresses 0x01-0x0a.
    fn map_pure_precompiles<F>(&mut self, f: F)
    where
        F: FnMut(&Address, DynPrecompile) -> DynPrecompile;
}

impl PrecompileMapExt for PrecompilesMap {
    fn map_pure_precompiles<F>(&mut self, mut f: F)
    where
        F: FnMut(&Address, DynPrecompile) -> DynPrecompile,
    {
        // Pure precompiles are at addresses 0x01 to 0x0a
        let pure_addresses: Vec<Address> = (1..=0xa)
            .map(|i| {
                let mut bytes = [0u8; 20];
                bytes[19] = i;
                Address::from(bytes)
            })
            .collect();

        // Use the existing map_precompiles method but only apply to pure addresses
        self.map_precompiles(|address, precompile| {
            if pure_addresses.contains(address) {
                f(address, precompile)
            } else {
                // Leave non-pure precompiles unchanged
                precompile
            }
        });
    }
}

/// Checks if the given address is a pure precompile address (0x01-0x0a)
#[allow(dead_code)]
fn is_pure_precompile(address: &Address) -> bool {
    let addr_bytes = address.as_slice();
    // Check if it's one of the standard Ethereum precompiles (0x01-0x0a)
    addr_bytes.len() == 20 &&
        addr_bytes[0..19].iter().all(|&b| b == 0) &&
        addr_bytes[19] >= 0x01 &&
        addr_bytes[19] <= 0x0a
}
