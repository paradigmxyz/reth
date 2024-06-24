//! Ethereum protocol-related constants

pub use reth_primitives_traits::constants::*;

/// Gas units, for example [`GIGAGAS`](gas_units::GIGAGAS).
pub mod gas_units;

/// [EIP-4844](https://eips.ethereum.org/EIPS/eip-4844#parameters) constants.
pub mod eip4844;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn min_protocol_sanity() {
        assert_eq!(MIN_PROTOCOL_BASE_FEE_U256.to::<u64>(), MIN_PROTOCOL_BASE_FEE);
    }
}
