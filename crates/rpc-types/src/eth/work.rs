use reth_primitives::{H256, U256};
use serde::{Serialize, Serializer};

/// The result of an `eth_getWork` request
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Work {
    /// The proof-of-work hash.
    pub pow_hash: H256,
    /// The seed hash.
    pub seed_hash: H256,
    /// The target.
    pub target: H256,
    /// The block number: this isn't always stored.
    pub number: Option<u64>,
}

impl Serialize for Work {
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self.number.as_ref() {
            Some(num) => {
                (&self.pow_hash, &self.seed_hash, &self.target, U256::from(*num)).serialize(s)
            }
            None => (&self.pow_hash, &self.seed_hash, &self.target).serialize(s),
        }
    }
}
