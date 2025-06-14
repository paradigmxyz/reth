mod builder;
mod config;
mod types;
#[cfg(test)]
mod tests;


pub use builder::{create_payload_builder_service, RollkitPayloadBuilder};
pub use config::{ConfigError, RollkitPayloadBuilderConfig};
pub use types::{RollkitPayloadAttributes, PayloadAttributesError};

pub fn add(left: u64, right: u64) -> u64 {
    left + right
}

#[cfg(test)]
mod lib_tests {
    use super::*;

    #[test]
    fn it_works() {
        let result = add(2, 2);
        assert_eq!(result, 4);
    }
}


