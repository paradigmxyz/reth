//! Curates the input coming from the fuzzer for certain types.

use reth_primitives_traits::IntegerList;
use serde::{Deserialize, Serialize};

/// Makes sure that the list provided by the fuzzer is not empty and pre-sorted
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct IntegerListInput(pub Vec<u64>);

impl From<IntegerListInput> for IntegerList {
    fn from(list: IntegerListInput) -> Self {
        let mut v = list.0;
        v.sort_unstable();
        Self::new_pre_sorted(v)
    }
}
