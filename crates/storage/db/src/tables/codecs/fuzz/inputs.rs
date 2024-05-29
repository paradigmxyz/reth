//! Curates the input coming from the fuzzer for certain types.

use reth_primitives::IntegerList;
use serde::{Deserialize, Serialize};

/// Makes sure that the list provided by the fuzzer is not empty and pre-sorted
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct IntegerListInput(pub Vec<u64>);

impl From<IntegerListInput> for IntegerList {
    fn from(list: IntegerListInput) -> IntegerList {
        let mut v = list.0;

        // Empty lists are not supported by `IntegerList`, so we want to skip these cases.
        if v.is_empty() {
            return vec![1u64].into()
        }
        v.sort();
        v.into()
    }
}
