use super::spec::BscSpecId;
use crate::chainspec::BscHardfork;
use alloy_primitives::BlockNumber;
use reth_chainspec::ChainSpec;
use reth_ethereum_forks::EthereumHardfork;

/// Returns the revm [`BscSpecId`] at the given timestamp.
///
/// # Note
///
/// This is only intended to be used after the Shangai, when hardforks are activated by
/// timestamp.
pub fn revm_spec_by_timestamp_after_shanghai(chain_spec: &ChainSpec, timestamp: u64) -> BscSpecId {
    if chain_spec.fork(BscHardfork::Bohr).active_at_timestamp(timestamp) {
        BscSpecId::BOHR
    } else if chain_spec.fork(BscHardfork::HaberFix).active_at_timestamp(timestamp) {
        BscSpecId::HABER_FIX
    } else if chain_spec.fork(BscHardfork::Haber).active_at_timestamp(timestamp) {
        BscSpecId::HABER
    } else if chain_spec.fork(BscHardfork::FeynmanFix).active_at_timestamp(timestamp) {
        BscSpecId::FEYNMAN_FIX
    } else if chain_spec.fork(BscHardfork::Feynman).active_at_timestamp(timestamp) {
        BscSpecId::FEYNMAN
    } else if chain_spec.fork(BscHardfork::Kepler).active_at_timestamp(timestamp) {
        BscSpecId::KEPLER
    } else {
        BscSpecId::SHANGHAI
    }
}

/// Returns the revm [`BscSpecId`] at the given block number.
///
/// # Note
///
/// This is only intended to be used before the Shangai, when hardforks are activated by
/// block number.
pub fn revm_spec(chain_spec: &ChainSpec, block: BlockNumber) -> BscSpecId {
    if chain_spec.fork(BscHardfork::Bohr).active_at_block(block) {
        BscSpecId::BOHR
    } else if chain_spec.fork(BscHardfork::HaberFix).active_at_block(block) {
        BscSpecId::HABER_FIX
    } else if chain_spec.fork(BscHardfork::Haber).active_at_block(block) {
        BscSpecId::HABER
    } else if chain_spec.fork(EthereumHardfork::Cancun).active_at_block(block) {
        BscSpecId::CANCUN
    } else if chain_spec.fork(BscHardfork::FeynmanFix).active_at_block(block) {
        BscSpecId::FEYNMAN_FIX
    } else if chain_spec.fork(BscHardfork::Feynman).active_at_block(block) {
        BscSpecId::FEYNMAN
    } else if chain_spec.fork(BscHardfork::Kepler).active_at_block(block) {
        BscSpecId::KEPLER
    } else if chain_spec.fork(EthereumHardfork::Shanghai).active_at_block(block) {
        BscSpecId::SHANGHAI
    } else if chain_spec.fork(BscHardfork::HertzFix).active_at_block(block) {
        BscSpecId::HERTZ_FIX
    } else if chain_spec.fork(BscHardfork::Hertz).active_at_block(block) {
        BscSpecId::HERTZ
    } else if chain_spec.fork(EthereumHardfork::London).active_at_block(block) {
        BscSpecId::LONDON
    } else if chain_spec.fork(EthereumHardfork::Berlin).active_at_block(block) {
        BscSpecId::BERLIN
    } else if chain_spec.fork(BscHardfork::Plato).active_at_block(block) {
        BscSpecId::PLATO
    } else if chain_spec.fork(BscHardfork::Luban).active_at_block(block) {
        BscSpecId::LUBAN
    } else if chain_spec.fork(BscHardfork::Planck).active_at_block(block) {
        BscSpecId::PLANCK
    } else if chain_spec.fork(BscHardfork::Gibbs).active_at_block(block) {
        // bsc mainnet and testnet have different order for Moran, Nano and Gibbs
        if chain_spec.fork(BscHardfork::Moran).active_at_block(block) {
            BscSpecId::MORAN
        } else if chain_spec.fork(BscHardfork::Nano).active_at_block(block) {
            BscSpecId::NANO
        } else {
            BscSpecId::EULER
        }
    } else if chain_spec.fork(BscHardfork::Moran).active_at_block(block) {
        BscSpecId::MORAN
    } else if chain_spec.fork(BscHardfork::Nano).active_at_block(block) {
        BscSpecId::NANO
    } else if chain_spec.fork(BscHardfork::Euler).active_at_block(block) {
        BscSpecId::EULER
    } else if chain_spec.fork(BscHardfork::Bruno).active_at_block(block) {
        BscSpecId::BRUNO
    } else if chain_spec.fork(BscHardfork::MirrorSync).active_at_block(block) {
        BscSpecId::MIRROR_SYNC
    } else if chain_spec.fork(BscHardfork::Niels).active_at_block(block) {
        BscSpecId::NIELS
    } else if chain_spec.fork(BscHardfork::Ramanujan).active_at_block(block) {
        BscSpecId::RAMANUJAN
    } else if chain_spec.fork(EthereumHardfork::MuirGlacier).active_at_block(block) {
        BscSpecId::MUIR_GLACIER
    } else if chain_spec.fork(EthereumHardfork::Istanbul).active_at_block(block) {
        BscSpecId::ISTANBUL
    } else if chain_spec.fork(EthereumHardfork::Petersburg).active_at_block(block) {
        BscSpecId::PETERSBURG
    } else if chain_spec.fork(EthereumHardfork::Constantinople).active_at_block(block) {
        BscSpecId::CONSTANTINOPLE
    } else if chain_spec.fork(EthereumHardfork::Byzantium).active_at_block(block) {
        BscSpecId::BYZANTIUM
    } else if chain_spec.fork(EthereumHardfork::Homestead).active_at_block(block) {
        BscSpecId::HOMESTEAD
    } else if chain_spec.fork(EthereumHardfork::Frontier).active_at_block(block) {
        BscSpecId::FRONTIER
    } else {
        panic!(
            "invalid hardfork chainspec: expected at least one hardfork, got {:?}",
            chain_spec.hardforks
        )
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::chainspec::bsc_chain_spec;

    #[test]
    fn test_revm_spec_by_timestamp_after_shanghai() {
        let chain_spec = bsc_chain_spec();

        let test_cases = [
            (1727317200, BscSpecId::BOHR),        // Bohr
            (1727316120, BscSpecId::HABER_FIX),   // Haber Fix
            (1718863500, BscSpecId::HABER),       // Haber
            (1713419340, BscSpecId::FEYNMAN_FIX), // Feynman Fix (checked before Feynman)
            (1705996800, BscSpecId::KEPLER),      // Kepler
        ];

        for (timestamp, expected_spec) in test_cases {
            let spec_id = revm_spec_by_timestamp_after_shanghai(&chain_spec, timestamp);
            assert_eq!(spec_id, expected_spec, "Failed at timestamp {}", timestamp);
        }

        let spec_id = revm_spec_by_timestamp_after_shanghai(&chain_spec, 0);
        assert_eq!(spec_id, BscSpecId::SHANGHAI, "Failed at default case");
    }
}
