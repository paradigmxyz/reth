use core::any::Any;

#[cfg(not(feature = "std"))]
use alloc::{format, string::String};

pub(crate) mod ethereum;
pub use ethereum::EthereumHardfork;

#[cfg(feature = "optimism")]
pub(crate) mod optimism;

/// Generic hardfork trait.
pub trait Hardfork: Any + CloneHardfork + Send + Sync + 'static {
    /// Fork name.
    fn name(&self) -> &'static str;
}

impl Clone for Box<dyn Hardfork> {
    fn clone(&self) -> Self {
        (**self).clone_box()
    }
}

pub trait CloneHardfork {
    fn clone_box(&self) -> Box<dyn Hardfork>;
}

impl<T> CloneHardfork for T
where
    T: 'static + Hardfork + Clone,
{
    fn clone_box(&self) -> Box<dyn Hardfork> {
        Box::new(self.clone())
    }
}

impl Hardfork for Box<dyn Hardfork> {
    /// Name of an hardfork.
    fn name(&self) -> &'static str {
        (**self).name()
    }
}

impl core::fmt::Debug for dyn Hardfork + 'static {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct(stringify!(self.name())).finish()
    }
}

impl PartialEq for dyn Hardfork + 'static {
    fn eq(&self, other: &Self) -> bool {
        self.name() == other.name()
    }
}

impl Eq for dyn Hardfork + 'static {}

/// Macro that defines different variants of a chain specific enum. See [`crate::Hardfork`] as an
/// example.
#[macro_export]
macro_rules! define_hardfork_enum {
    ($(#[$enum_meta:meta])* $enum:ident { $( $(#[$meta:meta])* $variant:ident ),* $(,)? }) => {
        $(#[$enum_meta])*
        #[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
        #[derive(Debug, Copy, Clone, Eq, PartialEq, PartialOrd, Ord, Hash)]
        #[non_exhaustive]
        pub enum $enum {
            $( $(#[$meta])* $variant ),*
        }

        impl $enum {
            /// Returns variant as `str`.
            pub const fn name(&self) -> &'static str {
                match self {
                    $( $enum::$variant => stringify!($variant), )*
                }
            }

            /// Boxes `self` and returns it as `Box<dyn Hardfork>`.
            pub fn boxed(self) -> Box<dyn Hardfork> {
                Box::new(self)
            }
        }

        impl FromStr for $enum {
            type Err = String;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s.to_lowercase().as_str() {
                    $( stringify!($variant) => Ok($enum::$variant), )*
                    _ => return Err(format!("Unknown hardfork: {s}")),
                }
            }
        }

        impl Hardfork for $enum {
            fn name(&self) -> &'static str {
                self.name()
            }
        }

        impl Display for $enum {
            fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
                write!(f, "{self:?}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[cfg(feature = "optimism")]
    use crate::hardfork::optimism::OptimismHardfork;

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
    #[cfg(feature = "optimism")]
    fn check_op_hardfork_from_str() {
        let hardfork_str = ["beDrOck", "rEgOlITH", "cAnYoN", "eCoToNe", "FJorD"];
        let expected_hardforks = [
            OptimismHardfork::Bedrock,
            OptimismHardfork::Regolith,
            OptimismHardfork::Canyon,
            OptimismHardfork::Ecotone,
            OptimismHardfork::Fjord,
        ];

        let hardforks: Vec<OptimismHardfork> =
            hardfork_str.iter().map(|h| OptimismHardfork::from_str(h).unwrap()).collect();

        assert_eq!(hardforks, expected_hardforks);
    }

    #[test]
    fn check_nonexistent_hardfork_from_str() {
        assert!(EthereumHardfork::from_str("not a hardfork").is_err());
    }
}
