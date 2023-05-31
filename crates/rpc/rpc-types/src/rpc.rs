use serde::{Deserialize, Serialize};
use std::collections::HashMap;
/// Represents the `rpc_modules` response, which returns the
/// list of all available modules on that transport and their version
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RPCModules {
    result: HashMap<String, String>,
}

impl Default for RPCModules {
    fn default() -> Self {
        Self {
            result: HashMap::from([
                ("txpool".to_owned(), "1.0".to_owned()),
                ("trace".to_owned(), "1.0".to_owned()),
                ("eth".to_owned(), "1.0".to_owned()),
                ("web3".to_owned(), "1.0".to_owned()),
                ("net".to_owned(), "1.0".to_owned()),
            ]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_parse_module_versions_roundtrip() {
        let s = r#"{"result":{"txpool":"1.0","trace":"1.0","eth":"1.0","web3":"1.0","net":"1.0"}}"#;
        let m: RPCModules = Default::default();

        let de_serialized: RPCModules = serde_json::from_str(&s).unwrap();
        assert_eq!(de_serialized, m);
    }
}
