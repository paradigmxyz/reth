//! Precompile map helpers.

use alloc::vec::Vec;

use alloy_primitives::Address;

pub use alloy_evm::precompiles::{DynPrecompile, Precompile, PrecompileInput, PrecompilesMap};

/// Error returned when moving precompiles.
#[derive(Debug, thiserror::Error)]
pub enum MovePrecompileError {
    /// The source address is not a precompile.
    #[error("account {0} is not a precompile")]
    NotAPrecompile(Address),
}

/// Interface for precompile maps that support moving precompile implementations.
pub trait PrecompileOverrideMap {
    /// Returns whether the address currently has a precompile.
    fn contains_precompile(&self, address: &Address) -> bool;

    /// Moves precompiles from their source addresses to destination addresses.
    fn move_precompiles(
        &mut self,
        moves: Vec<(Address, Address)>,
    ) -> Result<(), MovePrecompileError>;
}

impl PrecompileOverrideMap for PrecompilesMap {
    fn contains_precompile(&self, address: &Address) -> bool {
        self.get(address).is_some()
    }

    fn move_precompiles(
        &mut self,
        moves: Vec<(Address, Address)>,
    ) -> Result<(), MovePrecompileError> {
        Self::move_precompiles(self, moves).map_err(
            |alloy_evm::precompiles::MovePrecompileError::NotAPrecompile(address)| {
                MovePrecompileError::NotAPrecompile(address)
            },
        )
    }
}
