mod macros;

mod ethereum;
pub use ethereum::EthereumHardfork;

mod dev;
pub use dev::DEV_HARDFORKS;

use core::{
    any::Any,
    hash::{Hash, Hasher},
};
use dyn_clone::DynClone;

/// Generic hardfork trait.
#[auto_impl::auto_impl(&, Box)]
pub trait Hardfork: Any + DynClone + Send + Sync + 'static {
    /// Fork name.
    fn name(&self) -> &'static str;
}

dyn_clone::clone_trait_object!(Hardfork);

impl core::fmt::Debug for dyn Hardfork + 'static {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct(self.name()).finish()
    }
}

impl PartialEq for dyn Hardfork + 'static {
    fn eq(&self, other: &Self) -> bool {
        self.name() == other.name()
    }
}

impl Eq for dyn Hardfork + 'static {}

impl Hash for dyn Hardfork + 'static {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name().hash(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn check_hardfork_from_str() {
        let hardfork_str = [
            "frOntier",
            "homEstead",
            "dao",
            "tAngerIne",
            "spurIousdrAgon",
            "byzAntium",
            "constantinople",
            "petersburg",
            "istanbul",
            "muirglacier",
            "bErlin",
            "lonDon",
            "arrowglacier",
            "grayglacier",
            "PARIS",
            "ShAnGhAI",
            "CaNcUn",
            "PrAguE",
        ];
        let expected_hardforks = [
            EthereumHardfork::Frontier,
            EthereumHardfork::Homestead,
            EthereumHardfork::Dao,
            EthereumHardfork::Tangerine,
            EthereumHardfork::SpuriousDragon,
            EthereumHardfork::Byzantium,
            EthereumHardfork::Constantinople,
            EthereumHardfork::Petersburg,
            EthereumHardfork::Istanbul,
            EthereumHardfork::MuirGlacier,
            EthereumHardfork::Berlin,
            EthereumHardfork::London,
            EthereumHardfork::ArrowGlacier,
            EthereumHardfork::GrayGlacier,
            EthereumHardfork::Paris,
            EthereumHardfork::Shanghai,
            EthereumHardfork::Cancun,
            EthereumHardfork::Prague,
        ];

        let hardforks: Vec<EthereumHardfork> =
            hardfork_str.iter().map(|h| EthereumHardfork::from_str(h).unwrap()).collect();

        assert_eq!(hardforks, expected_hardforks);
    }

    #[test]
    fn check_nonexistent_hardfork_from_str() {
        assert!(EthereumHardfork::from_str("not a hardfork").is_err());
    }
}
