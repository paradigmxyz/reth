/// Macro that defines different variants of a chain specific enum. See [`crate::Hardfork`] as an
/// example.
#[macro_export]
macro_rules! hardfork {
    ($(#[$enum_meta:meta])* $enum:ident { $( $(#[$meta:meta])* $variant:ident ),* $(,)? }) => {
        $(#[$enum_meta])*
        #[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
        #[derive(Debug, Copy, Clone, Eq, PartialEq, PartialOrd, Ord, Hash)]
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
                    $(
                        s if s == stringify!($variant).to_lowercase() => Ok($enum::$variant),
                    )*
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
